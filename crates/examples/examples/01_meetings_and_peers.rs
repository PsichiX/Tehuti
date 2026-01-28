use examples::Example;
use std::time::Duration;
use tehuti::{
    channel::{ChannelId, ChannelMode},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{PeerFactory, PeerId, PeerRoleId},
};
use tehuti_mock::mock_recv_matching;

fn main() {
    let server_factory = PeerFactory::default().with(PeerRoleId::new(0), |builder| {
        // Server can only write to the channel.
        builder.bind_write::<String, String>(ChannelId::new(0), ChannelMode::ReliableOrdered, None)
    });
    let client_factory = PeerFactory::default().with(PeerRoleId::new(0), |builder| {
        // Client can only read from the channel.
        builder.bind_read::<String, String>(ChannelId::new(0), ChannelMode::ReliableOrdered, None)
    });

    Example::default()
        .machine(
            "server",
            Default::default(),
            server_factory.into(),
            Duration::ZERO,
            server,
        )
        .machine(
            "client",
            Default::default(),
            client_factory.into(),
            Duration::from_millis(100),
            client,
        )
        .join();
}

fn server(meeting: MeetingInterface) {
    // Make server create an authoritative peer.
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
}

fn client(meeting: MeetingInterface) {
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
}
