use examples::tcp_example_server;
use std::time::Duration;
use tehuti::{
    channel::{ChannelId, ChannelMode},
    event::Sender,
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{PeerFactory, PeerId, PeerRoleId},
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";

/// Simple example demonstrating a server and client exchanging messages
/// over a TCP connection using tehuti's meeting and peer abstractions.
fn main() {
    let factory = PeerFactory::default().with(PeerRoleId::new(0), |builder| {
        // Server can only write to the channel.
        builder.bind_write::<String, String>(ChannelId::new(0), ChannelMode::ReliableOrdered, None)
    });

    tcp_example_server(ADDRESS, factory.into(), server).unwrap();
}

fn server(meeting: MeetingInterface, terminate_sender: Sender<()>) {
    // Make server create an authoritative peer, that's replicated to clients.
    meeting
        .sender
        .send(MeetingUserEvent::PeerCreate(
            PeerId::new(0),
            PeerRoleId::new(0),
        ))
        .unwrap();

    // Wait for the peer to be added.
    let peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    );

    println!("* Server ready. Type messages to send to client, or 'exit' to quit.");

    loop {
        let mut command = String::new();
        std::io::stdin().read_line(&mut command).unwrap();

        // Send the command to the client over the channel.
        peer.send(ChannelId::new(0), command.to_owned()).unwrap();

        if command.trim() == "exit" {
            break;
        }
    }

    println!("* Server shutdown.");
    terminate_sender.send(()).unwrap();
}
