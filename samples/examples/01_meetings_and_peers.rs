use samples::tcp::tcp_example;
use std::{error::Error, net::SocketAddr, time::Duration};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    event::Duplex,
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{
        PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer, TypedPeerRole,
    },
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";
const MESSAGE_CHANNEL: ChannelId = ChannelId::new(0);

/// Simple example demonstrating a server and client exchanging messages
/// over a TCP connection using Tehuti's meeting and peer primitives.
fn main() -> Result<(), Box<dyn Error>> {
    println!("Are you hosting a server? (y/n): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let is_server = input.trim().to_lowercase() == "y";

    let factory = PeerFactory::default().with_typed::<ChatRole>();

    tcp_example(is_server, ADDRESS, factory.into(), app)?;
    Ok(())
}

fn app(
    is_server: bool,
    meeting: MeetingInterface,
    local_addr: SocketAddr,
) -> Result<(), Box<dyn Error>> {
    // Make server create an authoritative peer, that's replicated to clients.
    if is_server {
        meeting.sender.send(MeetingUserEvent::PeerCreate(
            PeerId::new(0),
            ChatRole::ROLE_ID,
        ))?;
    }

    // Wait for the peer (created by server) to be added.
    let peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    )
    .into_typed::<ChatRole>()?;

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
            sender
                .send(format!("{}: {}", local_addr, message.trim()).into())
                .unwrap();
        }
    });

    // Thread for receiving and printing messages.
    let output = std::thread::spawn(move || {
        loop {
            if let Ok(message) = receiver.recv_blocking() {
                println!("> {}", message.message.trim());
            } else {
                return;
            }
        }
    });

    // Cleanup.
    input.join().unwrap();
    output.join().unwrap();
    Ok(())
}

struct ChatRole {
    events: Duplex<Dispatch<String>>,
}

impl TypedPeerRole for ChatRole {
    const ROLE_ID: PeerRoleId = PeerRoleId::new(0);
}

impl TypedPeer for ChatRole {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        Ok(builder.bind_read_write::<String, String>(
            MESSAGE_CHANNEL,
            ChannelMode::ReliableOrdered,
            None,
        ))
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            events: peer.read_write(MESSAGE_CHANNEL)?,
        })
    }
}
