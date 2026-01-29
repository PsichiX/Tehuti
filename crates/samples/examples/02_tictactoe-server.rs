use examples::tcp_example_server;
use serde::{Deserialize, Serialize};
use std::{error::Error, str::FromStr, time::Duration};
use tehuti::{
    channel::{ChannelId, ChannelMode},
    codec::postcard::PostcardCodec,
    event::{Receiver, Sender},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer},
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";
const GAME_ROLE: PeerRoleId = PeerRoleId::new(0);
const MAKE_MOVE_CHANNEL: ChannelId = ChannelId::new(0);
const GAME_STATE_CHANNEL: ChannelId = ChannelId::new(1);

fn main() {
    let factory = PeerFactory::default().with(GAME_ROLE, GameRole::builder);

    tcp_example_server(ADDRESS, factory.into(), server).unwrap();
}

fn server(meeting: MeetingInterface, terminate_sender: Sender<()>) {
    // Make server create an authoritative game peer replicated to clients.
    meeting
        .sender
        .send(MeetingUserEvent::PeerCreate(PeerId::new(0), GAME_ROLE))
        .unwrap();

    println!("* Server ready.");
    println!("Type moves in format 'col,row' (e.g., '0,1') to make a move.");
    println!("Type 'exit' to quit.");

    let mut peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    )
    .into_typed::<GameRole>()
    .unwrap();

    println!("Initial game state:\n{}", peer.state);

    let mut is_your_turn = true;
    loop {
        if is_your_turn {
            println!("Your turn to make a move:");

            let mut command = String::new();
            std::io::stdin().read_line(&mut command).unwrap();

            if command.trim() == "exit" {
                break;
            }
            let event = match command.parse::<MakeMoveEvent>() {
                Ok(event) => event,
                Err(error) => {
                    println!("Error parsing move: {}", error);
                    continue;
                }
            };
            match peer.state.apply_move(event, Symbol::Cross) {
                Ok(()) => {
                    peer.game_state_events
                        .send(GameStateEvent::Update(peer.state))
                        .unwrap();
                    println!("Updated game state:\n{}", peer.state);
                }
                Err(error) => {
                    println!("Error applying move: {}", error);
                    continue;
                }
            }
        } else {
            println!("Waiting for opponent's move...");

            let event = peer.make_move_events.recv_blocking().unwrap();
            match peer.state.apply_move(event, Symbol::Circle) {
                Ok(()) => {
                    peer.game_state_events
                        .send(GameStateEvent::Update(peer.state))
                        .unwrap();
                    println!("Updated game state:\n{}", peer.state);
                }
                Err(error) => {
                    println!("Error applying move: {}", error);
                    peer.game_state_events.send(GameStateEvent::Retry).unwrap();
                    continue;
                }
            }
        }

        match peer.state.check_winner() {
            Some(true) => {
                println!("Crosses (X) win!");
                break;
            }
            Some(false) => {
                println!("Circles (O) win!");
                break;
            }
            None => {}
        }
        is_your_turn = !is_your_turn;
    }

    println!("* Server shutdown.");
    terminate_sender.send(()).unwrap();
}

struct GameRole {
    make_move_events: Receiver<MakeMoveEvent>,
    game_state_events: Sender<GameStateEvent>,
    state: GameState,
}

impl TypedPeer for GameRole {
    fn builder(builder: PeerBuilder) -> PeerBuilder {
        builder
            .bind_read::<PostcardCodec<MakeMoveEvent>, MakeMoveEvent>(
                MAKE_MOVE_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            )
            .bind_write::<PostcardCodec<GameStateEvent>, GameStateEvent>(
                GAME_STATE_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            )
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>>
    where
        Self: Sized,
    {
        Ok(Self {
            make_move_events: peer.read(MAKE_MOVE_CHANNEL)?,
            game_state_events: peer.write(GAME_STATE_CHANNEL)?,
            state: Default::default(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Symbol {
    Cross,
    Circle,
}

impl FromStr for Symbol {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "x" => Ok(Symbol::Cross),
            "o" => Ok(Symbol::Circle),
            _ => Err("Invalid symbol. Expected 'x' or 'o'.".into()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MakeMoveEvent {
    col: u8,
    row: u8,
}

impl FromStr for MakeMoveEvent {
    type Err = Box<dyn Error>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.trim().split(',').collect();
        if parts.len() != 2 {
            return Err("Invalid input format. Expected 'col,row'.".into());
        }
        Ok(MakeMoveEvent {
            col: parts[0].parse::<u8>()?.clamp(0, 2),
            row: parts[1].parse::<u8>()?.clamp(0, 2),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum GameStateEvent {
    Update(GameState),
    Retry,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
struct GameState {
    board: [[Option<Symbol>; 3]; 3],
}

impl GameState {
    fn apply_move(
        &mut self,
        make_move: MakeMoveEvent,
        symbol: Symbol,
    ) -> Result<(), Box<dyn Error>> {
        if self.board[make_move.row as usize][make_move.col as usize].is_some() {
            return Err("Cell is already occupied.".into());
        }
        self.board[make_move.row as usize][make_move.col as usize] = Some(symbol);
        Ok(())
    }

    fn check_winner(&self) -> Option<bool> {
        // Check rows and columns
        for i in 0..3 {
            if let Some(symbol) = self.board[i][0]
                && self.board[i][1] == Some(symbol)
                && self.board[i][2] == Some(symbol)
            {
                return Some(symbol == Symbol::Cross);
            }
            if let Some(symbol) = self.board[0][i]
                && self.board[1][i] == Some(symbol)
                && self.board[2][i] == Some(symbol)
            {
                return Some(symbol == Symbol::Cross);
            }
        }
        // Check diagonals
        if let Some(symbol) = self.board[0][0]
            && self.board[1][1] == Some(symbol)
            && self.board[2][2] == Some(symbol)
        {
            return Some(symbol == Symbol::Cross);
        }
        if let Some(symbol) = self.board[0][2]
            && self.board[1][1] == Some(symbol)
            && self.board[2][0] == Some(symbol)
        {
            return Some(symbol == Symbol::Cross);
        }
        None
    }
}

impl std::fmt::Display for GameState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for row in &self.board {
            for cell in row {
                let symbol = match cell {
                    Some(Symbol::Cross) => "X",
                    Some(Symbol::Circle) => "O",
                    None => ".",
                };
                write!(f, "{} ", symbol)?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}
