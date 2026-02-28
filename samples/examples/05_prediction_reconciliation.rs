use crossterm::{event::KeyCode, terminal::size};
use rand::random_range;
use samples::{tcp::tcp_example, terminal::Terminal, utils::Keys};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, error::Error, fmt::Write, net::SocketAddr};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    codec::postcard::PostcardCodec,
    event::{Duplex, Receiver, Sender, unbounded},
    meeting::MeetingInterface,
    peer::{
        Peer, PeerBuilder, PeerDestructurer, PeerId, PeerInfo, PeerRoleId, TypedPeer, TypedPeerRole,
    },
    replication::primitives::RepF32,
    third_party::{
        time::{Duration, Instant},
        tracing::debug,
    },
};
use tehuti_client_server::authority::{Authority, AuthorityUserData};
use tehuti_diagnostics::{log_buffer::LogBuffer, recorder::Recorder};
use tehuti_socket::TcpMeetingConfig;
use tehuti_timeline::{
    clock::{Clock, ClockEvent},
    history::{HistoryBuffer, HistoryEvent},
    time::TimeStamp,
};
use tracing_subscriber::{layer::SubscriberExt, registry, util::SubscriberInitExt};

const ADDRESS: &str = "127.0.0.1:8888";
const AUTHORITY_CLOCK_CHANNEL: ChannelId = ChannelId::new(10);
const PLAYER_INPUT_CHANNEL: ChannelId = ChannelId::new(0);
const PLAYER_STATE_CHANNEL: ChannelId = ChannelId::new(1);
const WORLD_SIZE: (i32, i32) = (20, 10);
const HISTORY_CAPACITY: usize = 16;
const SERVER_SEND_STATE_INTERVAL: Duration = Duration::from_millis(1000 / 20);
const CLIENT_SEND_INPUT_WINDOW: u64 = 4;
const CLIENT_LEAD_TICKS: u64 = 2;
const CLIENT_PING_INTERVAL: Duration = Duration::from_millis(250);
const DELTA_TIME: [Duration; 5] = [
    Duration::from_millis(1000 / 30),
    Duration::from_millis(1000 / 20),
    Duration::from_millis(1000 / 15),
    Duration::from_millis(100),
    Duration::from_millis(150),
];
const SPEED: f32 = 5.0;

type TimelineAuthority = Authority<ClockExtension>;

/// Example showing client-side prediction and server-side reconciliation of
/// player state in a simple game where players can move around in a world.
/// The server is authoritative and clients will predict their local player
/// state based on their inputs while waiting for authoritative state updates
/// from the server.
fn main() -> Result<(), Box<dyn Error>> {
    // Setup diagnostics.
    let log_buffer = LogBuffer::new(50);
    Terminal::set_global_log_buffer(log_buffer.clone());
    registry()
        .with(log_buffer.into_layer("debug"))
        .with(Recorder::new("./logs").into_layer("trace"))
        .init();

    println!("Are you hosting a server? (y/n): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let is_server = input.trim().to_lowercase() == "y";

    let factory = TimelineAuthority::peer_factory(is_server)?.with_typed::<PlayerRole>();

    tcp_example(
        is_server,
        ADDRESS,
        TcpMeetingConfig::default()
            .enable_all_warnings()
            .warn_unknown_channel_packet(false),
        factory.into(),
        app,
    )?;
    Ok(())
}

fn app(
    is_server: bool,
    meeting: MeetingInterface,
    local_addr: SocketAddr,
) -> Result<(), Box<dyn Error>> {
    println!("Starting game at local address: {}", local_addr);

    let mut game = Game::new(is_server, meeting)?;
    let mut terminal = Terminal::default();
    let mut keys = Keys::default();
    let mut frame_rate_mode = 0usize;

    // Game loop.
    loop {
        if !game.maintain()? {
            break;
        }

        // Update keys cache.
        let events = Terminal::events().collect::<Vec<_>>();
        keys.update(&events);

        if keys.get(KeyCode::Esc).just_pressed() {
            break;
        }

        // Adjust frame rate with ',' and '.' keys for simulating ping variation.
        if keys.get(KeyCode::Char(',')).just_pressed() {
            frame_rate_mode = frame_rate_mode.saturating_sub(1);
        }
        if keys.get(KeyCode::Char('.')).just_pressed() {
            frame_rate_mode = frame_rate_mode.saturating_add(1).min(DELTA_TIME.len() - 1);
        }

        let delta_time = DELTA_TIME[frame_rate_mode];

        game.prepare_current_tick()?;

        if game.authority.is_server() {
            game.maintain_server(delta_time.as_secs_f32())?;
        } else if game.authority.is_client() {
            game.maintain_client(delta_time.as_secs_f32())?;
        }

        game.handle_inputs(&keys);

        if game.authority.is_server() {
            // Server simulates single current frame as its clock is authoritative.
            game.simulate(delta_time.as_secs_f32());
        } else if game.authority.is_client()
            && let Clock::Simulation(clock) = &game.authority.extension().unwrap().clock
        {
            // Client simulates multiple frames if needed to catch up with the
            // estimated target tick that server is supposed to be at given time.
            let target = clock.estimate_target_tick(CLIENT_LEAD_TICKS);

            while game.authority.extension().unwrap().clock.tick() < target {
                game.simulate(delta_time.as_secs_f32());
            }
        }

        terminal.begin_draw(true);
        game.draw(&mut terminal);
        game.draw_debug(&mut terminal, &keys, delta_time);
        terminal.end_draw();

        std::thread::sleep(delta_time);
    }

    drop(game);
    Ok(())
}

// Container that holds communication authority, players and internal timer.
struct Game {
    authority: TimelineAuthority,
    added_peers_receiver: Receiver<Peer>,
    removed_peers_receiver: Receiver<PeerId>,
    players: HashMap<PeerId, PlayerRole>,
    timer: Instant,
}

impl Game {
    fn new(is_server: bool, meeting: MeetingInterface) -> Result<Self, Box<dyn Error>> {
        let (added_peers_sender, added_peers_receiver) = unbounded();
        let (removed_peers_sender, removed_peers_receiver) = unbounded();

        // Create communication authority that will handle client-server
        // communication, and hold relativistic clock.
        let mut authority =
            TimelineAuthority::new(is_server, meeting, added_peers_sender, removed_peers_sender)?;

        // Wait until authority is initialized for the sake of simpler code.
        while !authority.is_initialized() {
            authority.maintain()?;
            std::thread::sleep(Duration::from_millis(50));
        }

        // Create peer representing local player.
        authority.create_peer(PlayerRole::ROLE_ID)?;

        // If it's client, immediately do first ping to server for initial clock
        // synchronization.
        let extension = authority.extension_mut().unwrap();
        if let Clock::Simulation(clock) = &mut extension.clock {
            let event = clock.ping();
            extension.events.sender.send(event.into())?;
        }

        Ok(Self {
            authority,
            added_peers_receiver,
            removed_peers_receiver,
            players: Default::default(),
            timer: Instant::now(),
        })
    }

    fn maintain(&mut self) -> Result<bool, Box<dyn Error>> {
        self.authority.maintain()?;
        if !self.authority.is_initialized() {
            return Ok(false);
        }

        let current_tick = self.authority.extension().unwrap().clock.tick();

        // For every peer that showed up, create its player representation.
        for peer in self.added_peers_receiver.iter() {
            let mut player = peer.into_typed::<PlayerRole>()?;

            player.state_mut().set(
                current_tick,
                StateSnapshot {
                    position_x: RepF32(random_range(2..WORLD_SIZE.0 - 2) as f32),
                    position_y: RepF32(random_range(2..WORLD_SIZE.1 - 2) as f32),
                    velocity_x: RepF32(0.0),
                    velocity_y: RepF32(0.0),
                },
            );

            self.players.insert(player.info().peer_id, player);
        }

        for peer_id in self.removed_peers_receiver.iter() {
            self.players.remove(&peer_id);
        }

        Ok(true)
    }

    // After we enter new simulation frame, we should prepare this frame's
    // history buffers for all players by extrapolating from previous tick.
    fn prepare_current_tick(&mut self) -> Result<(), Box<dyn Error>> {
        let current_tick = self.authority.extension().unwrap().clock.tick();

        for player in self.players.values_mut() {
            if let Some(inputs) = player.inputs_mut() {
                inputs.ensure_timestamp(current_tick, Default::default);
            }
            player
                .state_mut()
                .ensure_timestamp(current_tick, Default::default);
        }

        Ok(())
    }

    fn maintain_server(&mut self, delta_time: f32) -> Result<(), Box<dyn Error>> {
        // Collect inputs from remote players and check for divergence with their
        // input history.
        let divergence = self.server_find_input_divergence()?;

        // If divergence happen, we must rollback and resimulate the game from
        // the last known compatible state up to current tick.
        if let Some(divergence) = divergence {
            self.resimulate(divergence, delta_time)?;
        }

        // Periodically send authoritative state updates to clients.
        if self.timer.elapsed() >= SERVER_SEND_STATE_INTERVAL {
            self.timer = Instant::now();
            self.server_send_state()?;
        }

        let extension = self.authority.extension().unwrap();

        // Handle relativistic clock synchronization when server receives ping
        // from client by responding with pong containing server's clock info.
        if let Clock::Authority(clock) = &extension.clock
            && let Some(Dispatch { message, .. }) = extension.events.receiver.last()
        {
            let event = clock.pong(message);
            extension.events.sender.send(event.into())?;
        }

        Ok(())
    }

    fn server_find_input_divergence(&mut self) -> Result<Option<TimeStamp>, Box<dyn Error>> {
        let mut divergence = None;

        // Check for divergence in inputs received from remote players. If any
        // received input history window diverged from local history, we need
        // to resimulate the game from the last known compatible tick.
        for player in self.players.values_mut() {
            if let PlayerRole::ServerRemote {
                input_receiver,
                input_history,
                ..
            } = player
                && let Some(Dispatch { message, .. }) = input_receiver.last()
            {
                let div = message.apply_history_divergence(input_history)?;
                divergence = TimeStamp::possibly_oldest(divergence, div);
            }
        }

        Ok(divergence)
    }

    fn server_send_state(&self) -> Result<(), Box<dyn Error>> {
        let current_tick = self.authority.extension().unwrap().clock.tick();

        for player in self.players.values() {
            let (sender, history) = match player {
                PlayerRole::ServerLocal {
                    state_sender,
                    state_history,
                    ..
                } => (state_sender, state_history),
                PlayerRole::ServerRemote {
                    state_sender,
                    state_history,
                    ..
                } => (state_sender, state_history),
                _ => {
                    continue;
                }
            };

            if let Some(event) = HistoryEvent::collect_snapshot(history, current_tick) {
                sender.send(event.into())?;
            }
        }

        Ok(())
    }

    fn maintain_client(&mut self, delta_time: f32) -> Result<(), Box<dyn Error>> {
        // Collect authoritative state updates from server and check for
        // divergence with client's predicted state. If any received state
        // update diverged from client's predicted state, we need to resimulate
        // the game from the last known compatible tick.
        let divergence = self.client_find_state_divergence()?;

        if let Some(divergence) = divergence {
            self.resimulate(divergence, delta_time)?;
        }

        let extension = self.authority.extension_mut().unwrap();

        if let Clock::Simulation(clock) = &mut extension.clock {
            // Handle relativistic clock synchronization by periodically pinging
            // the server.
            if self.timer.elapsed() >= CLIENT_PING_INTERVAL {
                let event = clock.ping();
                extension.events.sender.send(event.into())?;
                self.timer = Instant::now();
            }

            // Update client's clock based on pong info received from server.
            for Dispatch { message, .. } in extension.events.receiver.iter() {
                clock.roundtrip(message);
            }
        }

        Ok(())
    }

    fn client_find_state_divergence(&mut self) -> Result<Option<TimeStamp>, Box<dyn Error>> {
        let mut divergence = None;

        for player in self.players.values_mut() {
            match player {
                // For local player, find the oldest authoritative state desync
                // with client's predicted state, while applying received state
                // updates to client's history.
                PlayerRole::ClientLocal {
                    state_receiver,
                    state_history,
                    ..
                } => {
                    if let Some(Dispatch { message, .. }) = state_receiver.last() {
                        let div = message.apply_history_divergence(state_history)?;
                        divergence = TimeStamp::possibly_oldest(divergence, div);
                    }
                }
                // For remote player, simply apply received state updates to
                // client's history - there is no prediction for remote player
                // on client so we don't need to check for divergence here.
                PlayerRole::ClientRemote {
                    state_receiver,
                    state_snapshot,
                    ..
                } => {
                    if let Some(Dispatch { message, .. }) = state_receiver.last() {
                        message.apply_history(state_snapshot)?;
                    }
                }
                _ => {}
            }
        }

        Ok(divergence)
    }

    fn resimulate(&mut self, divergence: TimeStamp, delta_time: f32) -> Result<(), Box<dyn Error>> {
        let start_tick = divergence;
        let end_tick = self.authority.extension().unwrap().clock.tick();
        self.authority
            .extension_mut()
            .unwrap()
            .clock
            .set_tick(start_tick);

        // Rewind all players' input and state history buffers to the last known
        // compatible tick.
        for player in self.players.values_mut() {
            if let Some(inputs) = player.inputs_mut() {
                inputs.time_travel_to(start_tick);
            }
            player.state_mut().time_travel_to(start_tick);
        }

        // Simulate the game forward from the last known compatible tick up to
        // current tick. After resimulation, client's predicted state should be
        // corrected and in sync with the authoritative state.
        if start_tick < end_tick {
            debug!("Resimulating from tick {} to tick {}", start_tick, end_tick);

            while self.authority.extension().unwrap().clock.tick() < end_tick {
                self.simulate(delta_time);
            }

            self.server_send_state()?;
        }

        Ok(())
    }

    fn simulate(&mut self, delta_time: f32) {
        let current_tick = self.authority.extension().unwrap().clock.tick();

        // Grab current tick inputs and state for given player and simulate its
        // movement based on simple velocity and position integration.
        // REMEMBER: simulation should work on history buffer as much as possible!
        for player in self.players.values_mut() {
            let input = player.inputs().map(|history| {
                history
                    .get_extrapolated(current_tick)
                    .copied()
                    .unwrap_or_default()
            });

            let states = player.state_mut();

            let mut state = states
                .get_extrapolated(current_tick)
                .copied()
                .unwrap_or_default();

            if let Some(input) = input {
                state.velocity_x.0 = match (input.left, input.right) {
                    (true, false) => -SPEED,
                    (false, true) => SPEED,
                    _ => 0.0,
                };
                state.velocity_y.0 = match (input.up, input.down) {
                    (true, false) => -SPEED,
                    (false, true) => SPEED,
                    _ => 0.0,
                };
            }

            state.position_x.0 += state.velocity_x.0 * delta_time;
            state.position_y.0 += state.velocity_y.0 * delta_time;
            state.position_x.0 = state.position_x.0.clamp(0.0, (WORLD_SIZE.0 - 1) as f32);
            state.position_y.0 = state.position_y.0.clamp(0.0, (WORLD_SIZE.1 - 1) as f32);

            states.set(current_tick, state);
        }

        // Once simulation is done, advance the clock to move to next tick.
        self.authority.extension_mut().unwrap().clock.advance(1);
    }

    fn handle_inputs(&mut self, keys: &Keys) {
        let current_tick = self.authority.extension().unwrap().clock.tick();
        let input = InputSnapshot {
            left: keys.get(KeyCode::Left).is_down(),
            right: keys.get(KeyCode::Right).is_down(),
            up: keys.get(KeyCode::Up).is_down(),
            down: keys.get(KeyCode::Down).is_down(),
        };

        // Grab inputs state for local player and apply to current tick history
        // buffer. On client, also send inputs window to server for reconciliation.
        if let Some(player) = self.local_player_mut() {
            match player {
                PlayerRole::ServerLocal { input_history, .. } => {
                    input_history.set(current_tick, input);
                }
                PlayerRole::ClientLocal {
                    input_sender,
                    input_history,
                    ..
                } => {
                    input_history.set(current_tick, input);
                    let since = current_tick - CLIENT_SEND_INPUT_WINDOW;
                    if let Some(event) =
                        HistoryEvent::collect_history(input_history, since..=current_tick)
                    {
                        input_sender.send(event.into()).ok();
                    }
                }
                _ => {}
            }
        }
    }

    fn draw(&self, terminal: &mut Terminal) {
        // Draw world border.
        for x in 0..WORLD_SIZE.0 + 2 {
            terminal.display([x as u16, 0], "░");
        }
        for x in 0..WORLD_SIZE.0 + 2 {
            terminal.display([x as u16, WORLD_SIZE.1 as u16 + 1], "░");
        }
        for y in 0..WORLD_SIZE.1 + 2 {
            terminal.display([0, y as u16], "░");
        }
        for y in 0..WORLD_SIZE.1 + 2 {
            terminal.display([WORLD_SIZE.0 as u16 + 1, y as u16], "░");
        }

        let current_tick = self.authority.extension().unwrap().clock.tick();

        // Draw players as their peer id at current tick position.
        for player in self.players.values() {
            let state = player
                .state()
                .get_extrapolated(current_tick)
                .copied()
                .unwrap_or_default();
            let (x, y) = (state.position_x.0, state.position_y.0);

            if x >= 0.0 && y >= 0.0 && x < WORLD_SIZE.0 as f32 && y < WORLD_SIZE.1 as f32 {
                terminal.display(
                    [x as u16 + 1, y as u16 + 1],
                    player.info().peer_id.id().to_string(),
                );
            }
        }
    }

    fn draw_debug(&self, terminal: &mut Terminal, keys: &Keys, frame_rate: Duration) {
        if Terminal::global_log_buffer().is_some() {
            let (w, h) = size().unwrap();

            // Draw logs separator line.
            for x in 0..w {
                terminal.display([x, WORLD_SIZE.1 as u16 + 3], "█");
            }

            // Draw logs below the separator line.
            terminal.draw_global_log_buffer_region(
                [0, WORLD_SIZE.1 as u16 + 4],
                [w, h - WORLD_SIZE.1 as u16 - 4],
            );

            // Draw diagnostics separator line.
            for y in 0..WORLD_SIZE.1 as u16 + 3 {
                terminal.display([w - 30, y], "█");
            }

            // Draw diagnostics.
            let extension = self.authority.extension().unwrap();

            let mut diagnostics = String::new();
            writeln!(
                &mut diagnostics,
                "DeltaTime: {:?}\nPeers: {}\nTick: {}",
                frame_rate,
                self.players.len(),
                extension.clock.tick().ticks(),
            )
            .unwrap();

            if self.authority.is_server() {
                writeln!(
                    &mut diagnostics,
                    "State send interval: {:?}",
                    SERVER_SEND_STATE_INTERVAL
                )
                .unwrap();
            }

            if let Clock::Simulation(clock) = &extension.clock {
                writeln!(
                    &mut diagnostics,
                    "RTT: {:.2?}\nATR: {:.2?}",
                    clock.rtt(),
                    clock.authority_tick_rate(),
                )
                .unwrap();
            }

            writeln!(&mut diagnostics, "\nInput:").unwrap();
            if keys.get(KeyCode::Left).is_down() {
                writeln!(&mut diagnostics, "- LEFT").unwrap();
            }
            if keys.get(KeyCode::Right).is_down() {
                writeln!(&mut diagnostics, "- RIGHT").unwrap();
            }
            if keys.get(KeyCode::Up).is_down() {
                writeln!(&mut diagnostics, "- UP").unwrap();
            }
            if keys.get(KeyCode::Down).is_down() {
                writeln!(&mut diagnostics, "- DOWN").unwrap();
            }

            terminal.draw_text_region([w - 29, 0], [29, WORLD_SIZE.1 as u16 + 3], diagnostics);
        }
    }

    fn local_player_mut(&mut self) -> Option<&mut PlayerRole> {
        self.players
            .values_mut()
            .find(|player| !player.info().remote)
    }
}

// Type aliases for history events used to communicate inputs and state
// snapshots between server and clients.
// History events usually store only handful of last ticks of history, to make
// sure in case of loosing packets, we still obtain potentially missed history.
type InputEvent = HistoryEvent<InputSnapshot>;
type StateEvent = HistoryEvent<StateSnapshot>;

// Inputs snapshot should only contain input down states, from which simulation
// will deduce detailed changes between consecutive ticks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct InputSnapshot {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

// State snapshots should store information required for simulation evolution,
// so not only positions, but also velocities for example.
// If we will only send positions, players would not be able to predict
// their movement!
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct StateSnapshot {
    position_x: RepF32,
    position_y: RepF32,
    velocity_x: RepF32,
    velocity_y: RepF32,
}

// Player role channel rights:
// * Inputs window:
//   +----------------------+     +----------------------+
//   | Server               |     | Client               |
//   +--------+------+------+     +--------+------+------+
//   | Peer   | Send | Recv |     | Peer   | Send | Recv |
//   +--------+------+------+ <-> +--------+------+------+
//   | Local  | X    | X    |     | Remote | X    | X    |
//   | Remote | X    | V    |     | Local  | V    | X    |
//   +--------+------+------+     +--------+------+------+
//   Behavior:
//   - on server, only remote peer can receive inputs to apply to history buffer
//     and resimulate game if divergence is detected.
//   - on client, only local peer can send inputs to server for reconciliation.
// * State (authoritative snapshots / corrections):
//   +----------------------+     +----------------------+
//   | Server               |     | Client               |
//   +--------+------+------+     +--------+------+------+
//   | Peer   | Send | Recv |     | Peer   | Send | Recv |
//   +--------+------+------+ <-> +--------+------+------+
//   | Local  | V    | X    |     | Remote | X    | V    |
//   | Remote | V    | X    |     | Local  | X    | V    |
//   +--------+------+------+     +--------+------+------+
//   Behavior:
//   - on server, both local and remote peers can send authoritative snapshots
//     to clients.
//   - on client, both local and remote peers can receive authoritative snapshots
//     from server.
// * On server local and remote peers need input and state history for full
//   rollback and resimulation capabilities.
// * On client local peer needs input and state history for client-side prediction
//   and remote peer needs state snapshot for correction and extrapolation.
enum PlayerRole {
    ServerLocal {
        info: PeerInfo,
        state_sender: Sender<Dispatch<HistoryEvent<StateSnapshot>>>,
        input_history: HistoryBuffer<InputSnapshot>,
        state_history: HistoryBuffer<StateSnapshot>,
    },
    ServerRemote {
        info: PeerInfo,
        input_receiver: Receiver<Dispatch<HistoryEvent<InputSnapshot>>>,
        state_sender: Sender<Dispatch<HistoryEvent<StateSnapshot>>>,
        input_history: HistoryBuffer<InputSnapshot>,
        state_history: HistoryBuffer<StateSnapshot>,
    },
    ClientLocal {
        info: PeerInfo,
        input_sender: Sender<Dispatch<HistoryEvent<InputSnapshot>>>,
        state_receiver: Receiver<Dispatch<HistoryEvent<StateSnapshot>>>,
        input_history: HistoryBuffer<InputSnapshot>,
        state_history: HistoryBuffer<StateSnapshot>,
    },
    ClientRemote {
        info: PeerInfo,
        state_receiver: Receiver<Dispatch<HistoryEvent<StateSnapshot>>>,
        state_snapshot: HistoryBuffer<StateSnapshot>,
    },
}

impl PlayerRole {
    fn info(&self) -> &PeerInfo {
        match self {
            Self::ServerLocal { info, .. } => info,
            Self::ServerRemote { info, .. } => info,
            Self::ClientLocal { info, .. } => info,
            Self::ClientRemote { info, .. } => info,
        }
    }

    fn inputs(&self) -> Option<&HistoryBuffer<InputSnapshot>> {
        match self {
            Self::ServerLocal { input_history, .. } => Some(input_history),
            Self::ClientLocal { input_history, .. } => Some(input_history),
            Self::ServerRemote { input_history, .. } => Some(input_history),
            _ => None,
        }
    }

    fn inputs_mut(&mut self) -> Option<&mut HistoryBuffer<InputSnapshot>> {
        match self {
            Self::ServerLocal { input_history, .. } => Some(input_history),
            Self::ClientLocal { input_history, .. } => Some(input_history),
            Self::ServerRemote { input_history, .. } => Some(input_history),
            _ => None,
        }
    }

    fn state(&self) -> &HistoryBuffer<StateSnapshot> {
        match self {
            Self::ServerLocal { state_history, .. } => state_history,
            Self::ServerRemote { state_history, .. } => state_history,
            Self::ClientLocal { state_history, .. } => state_history,
            Self::ClientRemote { state_snapshot, .. } => state_snapshot,
        }
    }

    fn state_mut(&mut self) -> &mut HistoryBuffer<StateSnapshot> {
        match self {
            Self::ServerLocal { state_history, .. } => state_history,
            Self::ServerRemote { state_history, .. } => state_history,
            Self::ClientLocal { state_history, .. } => state_history,
            Self::ClientRemote { state_snapshot, .. } => state_snapshot,
        }
    }
}

impl TypedPeerRole for PlayerRole {
    const ROLE_ID: PeerRoleId = PeerRoleId::new(1);
}

impl TypedPeer for PlayerRole {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        let is_server = builder.user_data().access::<AuthorityUserData>()?.is_server;
        let is_local = !builder.info().remote;

        Ok(match (is_server, is_local) {
            (true, true) => builder.bind_write::<PostcardCodec<StateEvent>, StateEvent>(
                PLAYER_STATE_CHANNEL,
                ChannelMode::Unreliable,
                None,
            ),
            (true, false) => builder
                .bind_read::<PostcardCodec<InputEvent>, InputEvent>(
                    PLAYER_INPUT_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                )
                .bind_write::<PostcardCodec<StateEvent>, StateEvent>(
                    PLAYER_STATE_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                ),
            (false, true) => builder
                .bind_write::<PostcardCodec<InputEvent>, InputEvent>(
                    PLAYER_INPUT_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                )
                .bind_read::<PostcardCodec<StateEvent>, StateEvent>(
                    PLAYER_STATE_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                ),
            (false, false) => builder.bind_read::<PostcardCodec<StateEvent>, StateEvent>(
                PLAYER_STATE_CHANNEL,
                ChannelMode::Unreliable,
                None,
            ),
        })
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        let is_server = peer.user_data().access::<AuthorityUserData>()?.is_server;
        let is_local = !peer.info().remote;

        match (is_server, is_local) {
            (true, true) => Ok(Self::ServerLocal {
                info: *peer.info(),
                state_sender: peer.write::<StateEvent>(PLAYER_STATE_CHANNEL)?,
                input_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
            }),
            (true, false) => Ok(Self::ServerRemote {
                info: *peer.info(),
                input_receiver: peer.read::<InputEvent>(PLAYER_INPUT_CHANNEL)?,
                state_sender: peer.write::<StateEvent>(PLAYER_STATE_CHANNEL)?,
                input_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
            }),
            (false, true) => Ok(Self::ClientLocal {
                info: *peer.info(),
                input_sender: peer.write::<InputEvent>(PLAYER_INPUT_CHANNEL)?,
                state_receiver: peer.read::<StateEvent>(PLAYER_STATE_CHANNEL)?,
                input_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
            }),
            (false, false) => Ok(Self::ClientRemote {
                info: *peer.info(),
                state_receiver: peer.read::<StateEvent>(PLAYER_STATE_CHANNEL)?,
                state_snapshot: HistoryBuffer::with_capacity(1),
            }),
        }
    }
}

// Extension that holds relativistic clock synchronization.
// Extensions are arbitrary user-defined data that authority instance carries.
struct ClockExtension {
    clock: Clock,
    events: Duplex<Dispatch<ClockEvent>>,
}

impl TypedPeer for ClockExtension {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        Ok(
            builder.bind_read_write::<PostcardCodec<ClockEvent>, ClockEvent>(
                AUTHORITY_CLOCK_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            ),
        )
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        let is_server = peer.user_data().access::<AuthorityUserData>()?.is_server;
        let events = peer.read_write::<ClockEvent>(AUTHORITY_CLOCK_CHANNEL)?;

        Ok(Self {
            clock: if is_server {
                Clock::Authority(Default::default())
            } else {
                Clock::Simulation(Default::default())
            },
            events,
        })
    }
}
