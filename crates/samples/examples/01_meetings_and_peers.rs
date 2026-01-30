use examples::tcp::{tcp_example_client, tcp_example_server};
use std::{error::Error, time::Duration};
use tehuti::{
    channel::{ChannelId, ChannelMode},
    event::{Duplex, Sender},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer},
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";
const MESSAGE_CHANNEL: ChannelId = ChannelId::new(0);

/// Simple example demonstrating a server and client exchanging messages
/// over a TCP connection using Tehuti's meeting and peer abstractions.
fn main() {
    println!("Are you hosting a server? (y/n): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let server = input.trim().to_lowercase() == "y";

    let factory = PeerFactory::default().with_typed::<ChatRole>();

    if server {
        tcp_example_server(ADDRESS, factory.into(), app).unwrap();
    } else {
        tcp_example_client(ADDRESS, factory.into(), app).unwrap();
    }
}

fn app(is_server: bool, meeting: MeetingInterface, terminate_sender: Sender<()>) {
    // Make server create an authoritative peer, that's replicated to clients.
    if is_server {
        meeting
            .sender
            .send(MeetingUserEvent::PeerCreate(
                PeerId::new(0),
                ChatRole::ROLE_ID,
            ))
            .unwrap();
    }

    // Wait for the peer (created by server) to be added.
    let peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    )
    .into_typed::<ChatRole>()
    .unwrap();

    // Get the sender and receiver for chat messages on separate threads.
    let Duplex { sender, receiver } = peer.events;

    println!("You can start chatting now! Type 'exit' to quit.");

    // Thread for reading user input and sending messages.
    let input = std::thread::spawn(move || {
        let stdin = std::io::stdin();
        loop {
            let mut message = String::new();
            if stdin.read_line(&mut message).is_err() || message.trim().to_lowercase() == "exit" {
                return;
            }
            sender.send(message).unwrap();
        }
    });

    // Thread for receiving and printing messages.
    let output = std::thread::spawn(move || {
        loop {
            if let Ok(message) = receiver.recv_blocking() {
                println!(">>> {}", message);
            } else {
                return;
            }
        }
    });

    // Cleanup.
    input.join().unwrap();
    output.join().unwrap();
    terminate_sender.send(()).unwrap();
}

struct ChatRole {
    events: Duplex<String>,
}

impl TypedPeer for ChatRole {
    const ROLE_ID: PeerRoleId = PeerRoleId::new(0);

    fn builder(builder: tehuti::peer::PeerBuilder) -> tehuti::peer::PeerBuilder {
        builder.bind_read_write::<String, String>(
            MESSAGE_CHANNEL,
            ChannelMode::ReliableOrdered,
            None,
        )
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>>
    where
        Self: Sized,
    {
        Ok(Self {
            events: peer.read_write(MESSAGE_CHANNEL)?,
        })
    }
}
