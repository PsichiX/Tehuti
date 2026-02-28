use crossterm::{event::KeyCode, terminal::size};
use rand::random_range;
use samples::{tcp::tcp_example, terminal::Terminal, utils::Keys};
use std::{collections::HashMap, error::Error, net::SocketAddr, time::Duration};
use tehuti::{
    channel::{ChannelId, ChannelMode},
    codec::postcard::PostcardCodec,
    event::{Receiver, unbounded},
    meeting::MeetingInterface,
    peer::{Peer, PeerBuilder, PeerId, PeerRoleId, TypedPeer, TypedPeerRole},
    replica::{Replica, ReplicaApplyChanges, ReplicaCollectChanges, ReplicaId, ReplicationBuffer},
    replication::{HashReplicated, primitives::RepF32},
};
use tehuti_client_server::{
    authority::PureAuthority,
    controller::{Controller, ControllerEvent},
    puppet::{Puppet, Puppetable},
};
use tehuti_diagnostics::{log_buffer::LogBuffer, recorder::Recorder};
use tehuti_socket::TcpMeetingConfig;
use tracing_subscriber::{layer::SubscriberExt, registry, util::SubscriberInitExt};

const ADDRESS: &str = "127.0.0.1:8888";
const PLAYER_EVENT_CHANNEL: ChannelId = ChannelId::new(0);
const PLAYER_CHANGE_CHANNEL: ChannelId = ChannelId::new(1);
const WORLD_SIZE: (i32, i32) = (20, 10);
const DELTA_TIME: Duration = Duration::from_millis(1000 / 30);
const SPEED: f32 = 5.0;

/// Example showing replication of player state across machines in client-server
/// network architecture.
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

    let factory = PureAuthority::peer_factory(is_server)?.with_typed::<PlayerController>();

    tcp_example(
        is_server,
        ADDRESS,
        TcpMeetingConfig::default().enable_all_warnings(),
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

    // Main game loop.
    loop {
        if !game.tick()? {
            break;
        }

        let events = Terminal::events().collect::<Vec<_>>();
        keys.update(&events);
        if keys.get(KeyCode::Esc).just_pressed() {
            break;
        }

        game.handle_inputs(&keys);
        game.draw(&mut terminal);

        std::thread::sleep(DELTA_TIME);
    }

    // Don't let the game drop too early.
    drop(game);
    Ok(())
}

// Container holding game state such as communication authority and players.
struct Game {
    authority: PureAuthority,
    added_peers_receiver: Receiver<Peer>,
    removed_peers_receiver: Receiver<PeerId>,
    players: HashMap<PeerId, Player>,
}

impl Game {
    fn new(is_server: bool, meeting: MeetingInterface) -> Result<Self, Box<dyn Error>> {
        let (added_peers_sender, added_peers_receiver) = unbounded();
        let (removed_peers_sender, removed_peers_receiver) = unbounded();

        // Create authority and wait until it is initialized.
        let mut authority =
            PureAuthority::new(is_server, meeting, added_peers_sender, removed_peers_sender)?;

        while !authority.is_initialized() {
            authority.maintain()?;
            std::thread::sleep(Duration::from_millis(50));
        }

        // Create peer representing local player controller.
        authority.create_peer(PlayerController::ROLE_ID)?;

        Ok(Self {
            authority,
            added_peers_receiver,
            removed_peers_receiver,
            players: Default::default(),
        })
    }

    fn handle_inputs(&mut self, keys: &Keys) {
        // Make local character move using arrow keys.
        // Its position will be replicated to its peer on other side.
        if let Some(character) = self.local_character_mut() {
            if keys.get(KeyCode::Left).is_down() {
                character.position_x.0 -= SPEED * DELTA_TIME.as_secs_f32();
            }
            if keys.get(KeyCode::Right).is_down() {
                character.position_x.0 += SPEED * DELTA_TIME.as_secs_f32();
            }
            if keys.get(KeyCode::Up).is_down() {
                character.position_y.0 -= SPEED * DELTA_TIME.as_secs_f32();
            }
            if keys.get(KeyCode::Down).is_down() {
                character.position_y.0 += SPEED * DELTA_TIME.as_secs_f32();
            }
            character.position_x.0 = character.position_x.0.clamp(0.0, WORLD_SIZE.0 as f32 - 1.0);
            character.position_y.0 = character.position_y.0.clamp(0.0, WORLD_SIZE.1 as f32 - 1.0);
        }
    }

    fn draw(&self, terminal: &mut Terminal) {
        terminal.begin_draw(true);

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

        // Draw characters as their peer ID number.
        for (_, character) in self.characters() {
            let x = character.position_x.0;
            let y = character.position_y.0;

            if x >= 0.0 && y >= 0.0 && x < WORLD_SIZE.0 as f32 && y < WORLD_SIZE.1 as f32 {
                terminal.display(
                    [x as u16 + 1, y as u16 + 1],
                    character.peer_id.id().to_string(),
                );
            }
        }

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
        }

        terminal.end_draw();
    }

    fn tick(&mut self) -> Result<bool, Box<dyn Error>> {
        // Authority internally is just an event pump and so it needs to be
        // maintained every tick to work properly.
        self.authority.maintain()?;
        if !self.authority.is_initialized() {
            return Ok(false);
        }

        // Handle player peers added to meeting.
        for peer in self.added_peers_receiver.iter() {
            let (replica_added_sender, replica_added_receiver) = unbounded();
            let (replica_removed_sender, replica_removed_receiver) = unbounded();

            // Create controller for the peer.
            // Controllers are client-server representation of peers, that help
            // handle automated and correct replication across machines.
            let mut controller = Controller::new(
                peer,
                PLAYER_EVENT_CHANNEL,
                Some(PLAYER_CHANGE_CHANNEL),
                None,
                replica_added_sender,
                replica_removed_sender,
            )?;
            // Create replica for the controller to replicate its character state.
            controller.create_replica(&mut self.authority)?;

            // Request full snapshot replication of existing characters to new
            // controllers puppets on the other side to initialize their state.
            for player in self.players.values_mut() {
                if let Some(character) = &mut player.character {
                    character.request_full_snapshot();
                }
            }

            self.players.insert(
                controller.info().peer_id,
                Player {
                    controller,
                    character: None,
                    replica_added_receiver,
                    replica_removed_receiver,
                },
            );
        }

        // Handle player peers removed from meeting.
        for peer_id in self.removed_peers_receiver.iter() {
            self.players.remove(&peer_id);
        }

        // Tick all players to maintain their controllers and characters.
        for player in self.players.values_mut() {
            player.tick()?;
        }

        Ok(true)
    }

    fn characters(&self) -> impl Iterator<Item = (bool, &Puppet<Character>)> {
        self.players.values().filter_map(|player| {
            player
                .character
                .as_ref()
                .map(|puppet| (!player.controller.info().remote, puppet))
        })
    }

    fn local_character_mut(&mut self) -> Option<&mut Puppet<Character>> {
        self.players
            .values_mut()
            .filter_map(|player| {
                player
                    .character
                    .as_mut()
                    .map(|puppet| (!player.controller.info().remote, puppet))
            })
            .find(|(is_local, _)| *is_local)
            .map(|(_, character)| character)
    }
}

// Container holding player state such as controller and character puppet.
struct Player {
    controller: Controller,
    character: Option<Puppet<Character>>,
    replica_added_receiver: Receiver<Replica>,
    replica_removed_receiver: Receiver<ReplicaId>,
}

impl Player {
    fn tick(&mut self) -> Result<(), Box<dyn Error>> {
        self.controller.maintain()?;

        // Since our game assumes the only puppets controller can have is player
        // character, we can directly create puppet when we get any its replica.
        for replica in self.replica_added_receiver.iter() {
            // Create a new character puppet for player's replica and send its
            // initial full snapshot.
            // Puppets are client-server representation of replicas, that help
            // handle automated replication of their state across machines.
            self.character = Some(Puppet::new(
                replica,
                Character {
                    peer_id: self.controller.info().peer_id,
                    position_x: HashReplicated::new(RepF32(
                        random_range(2..WORLD_SIZE.0 - 2) as f32
                    )),
                    position_y: HashReplicated::new(RepF32(
                        random_range(2..WORLD_SIZE.1 - 2) as f32
                    )),
                },
            ));
        }

        // Since our game assumes the only puppets controller can have is player
        // character, we can directly remove puppet when we get any replica removed.
        for _ in self.replica_removed_receiver.iter() {
            self.character = None;
        }

        // Perform automatic replication for character puppet.
        if let Some(puppet) = &mut self.character {
            puppet.replicate()?;
        }

        Ok(())
    }
}

// Player controller role definition.
struct PlayerController;

impl TypedPeerRole for PlayerController {
    // Since role ID 0 is reserved for authority, we should start with 1.
    const ROLE_ID: PeerRoleId = PeerRoleId::new(1);
}

impl TypedPeer for PlayerController {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        if builder.info().remote {
            Ok(builder
                // Controller events should always be read and write, since they
                // are used for internal controller synchronization.
                .bind_read_write::<PostcardCodec<ControllerEvent>, ControllerEvent>(
                    PLAYER_EVENT_CHANNEL,
                    ChannelMode::ReliableOrdered,
                    None,
                )
                // Change channels should be read-only on remote for simulation.
                .bind_read::<ReplicationBuffer, ReplicationBuffer>(
                    PLAYER_CHANGE_CHANNEL,
                    ChannelMode::ReliableOrdered,
                    None,
                ))
        } else {
            Ok(builder
                .bind_read_write::<PostcardCodec<ControllerEvent>, ControllerEvent>(
                    PLAYER_EVENT_CHANNEL,
                    ChannelMode::ReliableOrdered,
                    None,
                )
                // Change channels should be write-only on local for authority.
                .bind_write::<ReplicationBuffer, ReplicationBuffer>(
                    PLAYER_CHANGE_CHANNEL,
                    ChannelMode::ReliableOrdered,
                    None,
                ))
        }
    }
}

// Character puppet definition.
struct Character {
    peer_id: PeerId,
    // Position is replicated across machines and automatically synchronized by
    // owning puppet.
    position_x: HashReplicated<RepF32>,
    position_y: HashReplicated<RepF32>,
}

impl Puppetable for Character {
    // Here we collect changes to be replicated to other machines.
    fn collect_changes(
        &mut self,
        mut collector: ReplicaCollectChanges,
        full_snapshot: bool,
    ) -> Result<(), Box<dyn Error>> {
        if full_snapshot {
            HashReplicated::mark_changed(&mut self.position_x);
            HashReplicated::mark_changed(&mut self.position_y);
        }
        collector
            .scope()
            .maybe_collect_replicated::<0, _, _>(&self.position_x)?;
        collector
            .scope()
            .maybe_collect_replicated::<1, _, _>(&self.position_y)?;
        Ok(())
    }

    // Here we apply changes replicated from other machines.
    fn apply_changes(&mut self, mut applicator: ReplicaApplyChanges) -> Result<(), Box<dyn Error>> {
        applicator
            .scope()
            .maybe_apply_replicated::<0, _, _>(&mut self.position_x)?;
        applicator
            .scope()
            .maybe_apply_replicated::<1, _, _>(&mut self.position_y)?;
        Ok(())
    }
}
