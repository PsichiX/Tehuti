use examples::tcp_example_client;
use std::time::Duration;
use tehuti::{
    channel::{ChannelId, ChannelMode},
    event::Sender,
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{PeerFactory, PeerRoleId},
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";

/// Simple example demonstrating a server and client exchanging messages
/// over a TCP connection using tehuti's meeting and peer abstractions.
fn main() {
    let factory = PeerFactory::default().with(PeerRoleId::new(0), |builder| {
        // Client can only read from the channel.
        builder.bind_read::<String, String>(ChannelId::new(0), ChannelMode::ReliableOrdered, None)
    });

    tcp_example_client(ADDRESS, factory.into(), client).unwrap();
}

fn client(meeting: MeetingInterface, terminate_sender: Sender<()>) {
    // Wait for the peer (created by server) to be added.
    let peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    );

    println!("* Client ready. Waiting for messages from server...");

    loop {
        // Receive message from the server over the channel.
        let message = peer.recv_blocking::<String>(ChannelId::new(0)).unwrap();
        println!("Client received:\n{}", message);

        if message.trim() == "exit" {
            break;
        }
    }

    println!("* Client shutdown.");
    terminate_sender.send(()).unwrap();
}
