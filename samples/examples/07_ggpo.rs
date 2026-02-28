use crossterm::{event::KeyCode, terminal::size};
use rand::{RngExt, SeedableRng, rngs::ChaCha8Rng};
use samples::{tcp::tcp_example, terminal::Terminal, utils::Keys};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::Write,
    net::SocketAddr,
    time::{Duration, Instant},
};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    codec::postcard::PostcardCodec,
    event::{Receiver, Sender},
    hash,
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{
        PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerInfo, PeerRoleId, TypedPeer,
        TypedPeerRole,
    },
    third_party::{rust_decimal::Decimal, tracing::debug},
};
use tehuti_diagnostics::{log_buffer::LogBuffer, recorder::Recorder};
use tehuti_socket::TcpMeetingConfig;
use tehuti_timeline::{
    history::{HistoryBuffer, HistoryEvent},
    time::TimeStamp,
};
use tracing_subscriber::{layer::SubscriberExt, registry, util::SubscriberInitExt};
use vek::num_traits::{FromPrimitive, ToPrimitive};

const ADDRESS: &str = "127.0.0.1:8888";
const INPUT_STATE_CHANNEL: ChannelId = ChannelId::new(0);
const STATE_HASH_CHANNEL: ChannelId = ChannelId::new(1);
const WORLD_SIZE: (i32, i32) = (20, 10);
const HISTORY_CAPACITY: usize = 16;
const SEND_STATE_HASH_INTERVAL: Duration = Duration::from_millis(200);
const SEND_INPUT_WINDOW: u64 = 8;
const INPUT_DELAY_TICKS: u64 = 3;
const MAX_PREDICTION_TICKS: u64 = 6;
const DELTA_TIME: Duration = Duration::from_millis(1000 / 30);
const SPEED: i64 = 5;
const DECIMAL_PRECISION: u32 = 6;

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

    let factory = PeerFactory::default().with_typed::<PlayerRole>();

    tcp_example(
        is_server,
        ADDRESS,
        TcpMeetingConfig::enable_all(),
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

    let mut game = Game::new(meeting, is_server)?;
    let mut terminal = Terminal::default();
    let mut keys = Keys::default();

    // After start we need to wait for other peer to join the game.
    // The reason for this is that game is fully deterministic, and only inputs
    // are synchronized between peers, no state at all. This means that if one
    // peer starts simulating the game before the other peer joins, they will
    // diverge immediately because one peer will have no inputs while the other
    // peer will have inputs from the start. In a real game you might want to
    // have a lobby or matchmaking system to handle this scenario reasonably.
    loop {
        game.maintain()?;

        if game.players.len() == 2 {
            break;
        }

        let events = Terminal::events().collect::<Vec<_>>();
        keys.update(&events);

        if keys.get(KeyCode::Esc).just_pressed() {
            return Ok(());
        }

        terminal.begin_draw(true);
        terminal.display([0, 0], "Waiting for other player to join...");
        terminal.end_draw();

        std::thread::sleep(DELTA_TIME);
    }

    // Initialize player states at tick 0 with deterministic random positions.
    // Game stores players in a b-tree map with peer ID being the key, so the
    // order of players is guaranteed to be the same across peers, which means
    // that the random positions will also be the same across peers as both
    // peers use the same deterministic random number generator seed.
    for player in game.players.values_mut() {
        player.state_history.set(
            game.current_tick,
            StateSnapshot {
                position_x: i64_to_decimal(game.rng.random_range(2..WORLD_SIZE.0 - 2) as i64),
                position_y: i64_to_decimal(game.rng.random_range(2..WORLD_SIZE.1 - 2) as i64),
                velocity_x: i64_to_decimal(0),
                velocity_y: i64_to_decimal(0),
            },
        );
    }

    // Game loop.
    loop {
        game.maintain()?;

        // Whenever either peer leaves the game, we stop the game. In a real
        // game you might want to handle this scenario more gracefully, like
        // showing a message that the other player has left and waiting for
        // them to rejoin or finding a new opponent, instead of just stopping
        // the game immediately.
        if game.players.len() < 2 {
            break;
        }

        // Update keys cache.
        let events = Terminal::events().collect::<Vec<_>>();
        keys.update(&events);

        if keys.get(KeyCode::Esc).just_pressed() {
            break;
        }

        game.prepare_current_tick()?;

        game.handle_remote_inputs()?;

        game.handle_local_inputs(&keys);

        game.progress_forward();

        if game.detect_state_desync()? {
            break;
        }

        terminal.begin_draw(true);
        game.draw(&mut terminal);
        game.draw_debug(&mut terminal, &keys);
        terminal.end_draw();

        std::thread::sleep(DELTA_TIME);
    }

    drop(game);
    Ok(())
}

struct Game {
    meeting: MeetingInterface,
    current_tick: TimeStamp,
    // Notice that players are stored in a b-tree map instead of hashmap.
    // This ensures that the iteration order of players is consistent across
    // all machines, which is important for deterministic simulation!
    players: BTreeMap<PeerId, PlayerRole>,
    timer: Instant,
    rng: ChaCha8Rng,
}

impl Game {
    fn new(meeting: MeetingInterface, is_server: bool) -> Result<Self, Box<dyn Error>> {
        // Create player peer representing the local player.
        // For the sake of example, we just assume hosting peer has id 0 and
        // joining peer has id 1, but in a real game you would probably want
        // to have a more robust system for assigning peer IDs, like having
        // the host assign them when peers connect, instead of hardcoding them.
        meeting.sender.send(MeetingUserEvent::PeerCreate(
            PeerId::new(if is_server { 0 } else { 1 }),
            PlayerRole::ROLE_ID,
        ))?;

        Ok(Self {
            meeting,
            current_tick: Default::default(),
            players: Default::default(),
            timer: Instant::now(),
            rng: ChaCha8Rng::from_seed([0; 32]),
        })
    }

    fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        // Handle meeting events.
        for event in self.meeting.receiver.iter() {
            match event {
                MeetingUserEvent::PeerAdded(peer) => {
                    let peer = peer.into_typed::<PlayerRole>()?;
                    self.players.insert(peer.info.peer_id, peer);
                }
                MeetingUserEvent::PeerRemoved(peer_id) => {
                    self.players.remove(&peer_id);
                }
                _ => {}
            }
        }

        Ok(())
    }

    // Preparing current tick means all history buffers have been extrapolated
    // from past ticks so if some data won't set any change at given tick,
    // history will use past data to fill the gap.
    fn prepare_current_tick(&mut self) -> Result<(), Box<dyn Error>> {
        for player in self.players.values_mut() {
            player
                .input_history
                .ensure_timestamp(self.current_tick, Default::default);
            player
                .state_history
                .ensure_timestamp(self.current_tick, Default::default);
            player
                .state_hash_history
                .ensure_timestamp(self.current_tick, Default::default);
        }

        Ok(())
    }

    // Handling remote inputs means receiving input events from remote peers and
    // applying them to the input history. If there is any divergence in input
    // history in regards to applied history from given peer authority, we need
    // to resimulate the game from the divergence point to ensure that the game
    // state is consistent across all machines for simulation correctness.
    fn handle_remote_inputs(&mut self) -> Result<(), Box<dyn Error>> {
        let mut divergence = None;

        for player in self.players.values_mut() {
            if let PlayerCommunication::Remote { input_receiver, .. } = &player.communication {
                for Dispatch { message, .. } in input_receiver.iter() {
                    player.confirmed_tick = player.confirmed_tick.max(message.now());
                    let div = message.apply_history_divergence(&mut player.input_history)?;
                    divergence = TimeStamp::possibly_oldest(divergence, div);
                }
            }
        }

        if let Some(divergence) = divergence {
            self.resimulate(divergence)?;
        }

        Ok(())
    }

    fn handle_local_inputs(&mut self, keys: &Keys) {
        let input_tick = self.current_tick + INPUT_DELAY_TICKS;
        let input = InputSnapshot {
            left: keys.get(KeyCode::Left).is_down(),
            right: keys.get(KeyCode::Right).is_down(),
            up: keys.get(KeyCode::Up).is_down(),
            down: keys.get(KeyCode::Down).is_down(),
        };

        if let Some(player) = self.local_player_mut()
            && let PlayerCommunication::Local { input_sender, .. } = &player.communication
        {
            player.input_history.set(input_tick, input);
            let since = input_tick - SEND_INPUT_WINDOW;
            if let Some(event) =
                HistoryEvent::collect_history(&player.input_history, since..=input_tick)
            {
                input_sender.send(event.into()).ok();
            }
        }
    }

    fn resimulate(&mut self, divergence: TimeStamp) -> Result<(), Box<dyn Error>> {
        let end_tick = self.current_tick;
        self.current_tick = divergence;

        for player in self.players.values_mut() {
            player.input_history.time_travel_to(divergence);
            player.state_history.time_travel_to(divergence);
            player.state_hash_history.time_travel_to(divergence);
        }

        if self.current_tick < end_tick {
            debug!("Resimulating from tick {} to tick {}", divergence, end_tick);

            while self.current_tick < end_tick {
                self.simulate();
            }
        }

        Ok(())
    }

    // Simulating the game means applying inputs to the game state and moving
    // the game forward by one tick. This function should be deterministic,
    // meaning that given the same game state and inputs, it should always
    // produce the same results on all machines.
    fn simulate(&mut self) {
        let delta_time = duration_to_decimal(DELTA_TIME);

        for player in self.players.values_mut() {
            // Ensure that history buffers have entries for the current tick.
            // The reason we do this here and not only in frame preparation is
            // that simulation steps can be also performed by resimulation,
            // and since we borrow player state mutably, we can't extrapolate
            // history at the same time, so we need to make sure that history
            // buffers data is available for the current tick before we simulate.
            player
                .input_history
                .ensure_timestamp(self.current_tick, Default::default);
            player
                .state_history
                .ensure_timestamp(self.current_tick, Default::default);
            player
                .state_hash_history
                .ensure_timestamp(self.current_tick, Default::default);

            let input = player
                .input_history
                .get(self.current_tick)
                .copied()
                .unwrap();

            let state = player.state_history.get_mut(self.current_tick).unwrap();

            state.velocity_x = match (input.left, input.right) {
                (true, false) => i64_to_decimal(-SPEED),
                (false, true) => i64_to_decimal(SPEED),
                _ => i64_to_decimal(0),
            };
            state.velocity_y = match (input.up, input.down) {
                (true, false) => i64_to_decimal(-SPEED),
                (false, true) => i64_to_decimal(SPEED),
                _ => i64_to_decimal(0),
            };

            state.position_x += state.velocity_x * delta_time;
            state.position_y += state.velocity_y * delta_time;
            state.position_x = state
                .position_x
                .clamp(i64_to_decimal(0), i64_to_decimal(WORLD_SIZE.0 as i64 - 1));
            state.position_y = state
                .position_y
                .clamp(i64_to_decimal(0), i64_to_decimal(WORLD_SIZE.1 as i64 - 1));
        }

        self.current_tick += 1;
    }

    // Target tick is the tick that we want to simulate up to.
    // In an ideal world, this would always be next tick, but in case we don't
    // receive inputs from remote peers in time, we might need to predict up to
    // couple of frames since last confirmed tick in all peers.
    fn target_tick(&self) -> TimeStamp {
        let min_confirmed_tick = self
            .players
            .values()
            .filter_map(|player| {
                if player.info.remote {
                    Some(player.confirmed_tick)
                } else {
                    None
                }
            })
            .min()
            .unwrap_or(self.current_tick);

        (min_confirmed_tick + MAX_PREDICTION_TICKS).min(self.current_tick + 1)
    }

    fn progress_forward(&mut self) {
        let target_tick = self.target_tick();

        if self.current_tick < target_tick {
            while self.current_tick < target_tick {
                self.simulate();
            }
        } else {
            debug!(
                "Current tick {} is too far from target tick {}, not progressing",
                self.current_tick, target_tick
            );
        }
    }

    // Detecting state desync means comparing state hashes of all peers at the
    // current tick and checking if they match. If there is a mismatch, it means
    // that there is a desync in the game state between peers, which means either
    // simulation isn't fully deterministic, or some player is cheating.
    fn detect_state_desync(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut desync = false;

        for player in self.players.values_mut() {
            let Some(state_hash) = player
                .state_history
                .get(self.current_tick)
                .map(|state| hash(&state))
            else {
                continue;
            };

            player.state_hash_history.set(self.current_tick, state_hash);

            if self.timer.elapsed() >= SEND_STATE_HASH_INTERVAL {
                self.timer = Instant::now();

                if let PlayerCommunication::Local {
                    state_hash_sender, ..
                } = &player.communication
                    && let Some(event) = HistoryEvent::collect_history(
                        &player.state_hash_history,
                        self.current_tick..=self.current_tick,
                    )
                {
                    state_hash_sender.send(event.into()).ok();
                }
            }

            if let PlayerCommunication::Remote {
                state_hash_receiver,
                ..
            } = &player.communication
            {
                for Dispatch { message, .. } in state_hash_receiver.iter() {
                    for (tick, hash) in message.iter() {
                        let Some(local_hash) = player.state_hash_history.get(tick).copied() else {
                            continue;
                        };

                        if *hash != local_hash {
                            debug!(
                                "State hash mismatch at tick {}: local = {}, remote = {}",
                                tick, local_hash, hash
                            );
                            desync = true;
                        }
                    }
                }
            }
        }

        Ok(desync)
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

        // Draw players as their peer id at current tick position.
        for player in self.players.values() {
            let state = player
                .state_history
                .get_extrapolated(self.current_tick)
                .copied()
                .unwrap_or_default();
            let (x, y) = (
                state.position_x.to_i32().unwrap_or_default(),
                state.position_y.to_i32().unwrap_or_default(),
            );

            if x >= 0 && y >= 0 && x < WORLD_SIZE.0 && y < WORLD_SIZE.1 {
                terminal.display(
                    [x as u16 + 1, y as u16 + 1],
                    player.info.peer_id.id().to_string(),
                );
            }
        }
    }

    fn draw_debug(&self, terminal: &mut Terminal, keys: &Keys) {
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
            let mut diagnostics = String::new();
            writeln!(
                &mut diagnostics,
                "DeltaTime: {:?}\nPeers: {}\nTick: {}",
                DELTA_TIME,
                self.players.len(),
                self.current_tick,
            )
            .unwrap();

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
        self.players.values_mut().find(|player| !player.info.remote)
    }
}

type InputEvent = HistoryEvent<InputSnapshot>;
type StateHashEvent = HistoryEvent<u64>;

// History snapshot of a player input used to derive simulation state changes.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct InputSnapshot {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

// History snapshot of a player state.
// Notice we use Decimal type for positions and velocities instead of floating
// point numbers. This is because floating point numbers can have different
// precision and rounding behaviour on different machines, which can lead to
// desyncs in the game state - they aren't guaranteed to be deterministic.
// For our game we force all decimals to have specific precision.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize, Hash)]
struct StateSnapshot {
    position_x: Decimal,
    position_y: Decimal,
    velocity_x: Decimal,
    velocity_y: Decimal,
}

// Channels used for peer communication. Local player can send input and state
// hash events, while remote player can receive them.
enum PlayerCommunication {
    Local {
        input_sender: Sender<Dispatch<InputEvent>>,
        state_hash_sender: Sender<Dispatch<StateHashEvent>>,
    },
    Remote {
        input_receiver: Receiver<Dispatch<InputEvent>>,
        state_hash_receiver: Receiver<Dispatch<StateHashEvent>>,
    },
}

struct PlayerRole {
    info: PeerInfo,
    communication: PlayerCommunication,
    input_history: HistoryBuffer<InputSnapshot>,
    state_history: HistoryBuffer<StateSnapshot>,
    state_hash_history: HistoryBuffer<u64>,
    confirmed_tick: TimeStamp,
}

impl TypedPeerRole for PlayerRole {
    const ROLE_ID: PeerRoleId = PeerRoleId::new(0);
}

impl TypedPeer for PlayerRole {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        if builder.info().remote {
            Ok(builder
                .bind_read::<PostcardCodec<InputEvent>, InputEvent>(
                    INPUT_STATE_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                )
                .bind_read::<PostcardCodec<StateHashEvent>, StateHashEvent>(
                    STATE_HASH_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                ))
        } else {
            Ok(builder
                .bind_write::<PostcardCodec<InputEvent>, InputEvent>(
                    INPUT_STATE_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                )
                .bind_write::<PostcardCodec<StateHashEvent>, StateHashEvent>(
                    STATE_HASH_CHANNEL,
                    ChannelMode::Unreliable,
                    None,
                ))
        }
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        if peer.info().remote {
            Ok(Self {
                info: *peer.info(),
                communication: PlayerCommunication::Remote {
                    input_receiver: peer.read::<InputEvent>(INPUT_STATE_CHANNEL)?,
                    state_hash_receiver: peer.read::<StateHashEvent>(STATE_HASH_CHANNEL)?,
                },
                input_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_hash_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                confirmed_tick: Default::default(),
            })
        } else {
            Ok(Self {
                info: *peer.info(),
                communication: PlayerCommunication::Local {
                    input_sender: peer.write::<InputEvent>(INPUT_STATE_CHANNEL)?,
                    state_hash_sender: peer.write::<StateHashEvent>(STATE_HASH_CHANNEL)?,
                },
                input_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                state_hash_history: HistoryBuffer::with_capacity(HISTORY_CAPACITY),
                confirmed_tick: Default::default(),
            })
        }
    }
}

fn duration_to_decimal(value: Duration) -> Decimal {
    let mut result = Decimal::from_u128(value.as_micros()).unwrap_or_default()
        / Decimal::from_u64(1_000_000).unwrap();
    result.rescale(DECIMAL_PRECISION);
    result
}

fn i64_to_decimal(value: i64) -> Decimal {
    let mut result = Decimal::from_i64(value).unwrap_or_default();
    result.rescale(DECIMAL_PRECISION);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_to_decimal() {
        let duration = Duration::new(1, 500_000_000);
        let decimal = duration_to_decimal(duration);
        assert_eq!(decimal, Decimal::from_str_exact("1.500000").unwrap());

        let duration = Duration::from_millis(16);
        let decimal = duration_to_decimal(duration);
        assert_eq!(decimal, Decimal::from_str_exact("0.016000").unwrap());

        let duration = Duration::from_millis(33);
        let decimal = duration_to_decimal(duration);
        assert_eq!(decimal, Decimal::from_str_exact("0.033000").unwrap());
    }
}
