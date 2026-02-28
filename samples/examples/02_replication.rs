use samples::tcp::tcp_example;
use std::{error::Error, net::SocketAddr, thread::sleep};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    event::{Receiver, Sender},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{
        PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer, TypedPeerRole,
    },
    replica::{Replica, ReplicaId, ReplicaSet, ReplicationBuffer},
    replication::HashReplicated,
    third_party::time::{Duration, Instant},
};
use tehuti_mock::mock_recv_matching;
use tehuti_socket::TcpMeetingConfig;

const ADDRESS: &str = "127.0.0.1:8888";
const CHANGE_CHANNEL: ChannelId = ChannelId::new(0);

/// Simple example demonstrating replication of a counter from an authority
/// peer (server) to a simulated peer (client) over a TCP connection using
/// Tehuti's meeting, peer, and replication primitives.
fn main() -> Result<(), Box<dyn Error>> {
    println!("Are you hosting a server? (y/n): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let is_server = input.trim().to_lowercase() == "y";

    let factory = PeerFactory::default()
        .with_user_data(is_server)?
        .with_typed::<CounterRole>();

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
    _local_addr: SocketAddr,
) -> Result<(), Box<dyn Error>> {
    if is_server {
        meeting.sender.send(MeetingUserEvent::PeerCreate(
            PeerId::new(0),
            CounterRole::ROLE_ID,
        ))?;
    }

    // Wait for the peer (created by server) to be added.
    let peer = mock_recv_matching!(
        meeting.receiver,
        Duration::from_secs(1),
        MeetingUserEvent::PeerAdded(peer) => peer
    )
    .into_typed::<CounterRole>()?;

    // Create a replica set and bind it to the peer's replication channels.
    let mut replica_set = ReplicaSet::default();
    match &peer {
        CounterRole::Authority { changes } => {
            replica_set.bind_change_sender(changes.clone());
        }
        CounterRole::Simulated { changes } => {
            replica_set.bind_change_receiver(changes.clone());
        }
    }

    // Create a counter replica.
    let mut counter = Counter {
        replica: replica_set.create(ReplicaId::new(0))?,
        value: HashReplicated::new(0),
    };

    let mut timer = Instant::now();
    loop {
        // Authority peer: increment the counter every second.
        if timer.elapsed() >= Duration::from_secs(1) {
            timer = Instant::now();

            if matches!(&peer, CounterRole::Authority { .. }) {
                *counter.value += 1;

                println!("Tick counter: {}", *counter.value);
            }
        }

        // Authority peer: replicate the change to the simulated peer.
        if let Some(mut collector) = counter.replica.collect_changes() {
            collector
                .scope()
                .collect_replicated(&counter.value)
                .unwrap();
        }

        // Simulated peer: replicate changes from the authority peer.
        if let Some(mut applicator) = counter.replica.apply_changes() {
            applicator
                .scope()
                .apply_replicated(&mut counter.value)
                .unwrap();

            println!("Counter ticked: {}", *counter.value);
        }

        replica_set.maintain();

        sleep(Duration::from_millis(100));
    }
}

// Define the peer role with channels access mode based on whether it's an
// authority or simulated peer.
// Authority peers can only write replication, while simulated peers can only
// read replication.
enum CounterRole {
    Authority {
        changes: Sender<Dispatch<ReplicationBuffer>>,
    },
    Simulated {
        changes: Receiver<Dispatch<ReplicationBuffer>>,
    },
}

impl TypedPeerRole for CounterRole {
    const ROLE_ID: PeerRoleId = PeerRoleId::new(0);
}

impl TypedPeer for CounterRole {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        let is_server = builder.user_data().access::<bool>().copied()?;

        if is_server {
            Ok(builder.bind_write::<ReplicationBuffer, ReplicationBuffer>(
                CHANGE_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            ))
        } else {
            Ok(builder.bind_read::<ReplicationBuffer, ReplicationBuffer>(
                CHANGE_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            ))
        }
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        let is_server = *peer.user_data().access::<bool>()?;

        if is_server {
            Ok(Self::Authority {
                changes: peer.write::<ReplicationBuffer>(CHANGE_CHANNEL)?,
            })
        } else {
            Ok(Self::Simulated {
                changes: peer.read::<ReplicationBuffer>(CHANGE_CHANNEL)?,
            })
        }
    }
}

struct Counter {
    // The replica representing this counter.
    replica: Replica,
    // The replicated counter value.
    value: HashReplicated<u32>,
}
