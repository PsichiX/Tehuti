use samples::tcp::tcp_example;
use std::{
    error::Error,
    net::SocketAddr,
    thread::sleep,
    time::{Duration, Instant},
};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    codec::replicable::RepCodec,
    event::{Receiver, Sender},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{
        PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer, TypedPeerRole,
    },
    replica::{Replica, ReplicaId, ReplicaSet, ReplicationBuffer},
    replication::rpc::Rpc,
};
use tehuti_mock::mock_recv_matching;

const ADDRESS: &str = "127.0.0.1:8888";
const RPC_CHANNEL: ChannelId = ChannelId::new(0);

type TickRpc = Rpc<(), RepCodec<u32>>;

/// Simple example demonstrating replication of a counter from an authority
/// peer (server) to a simulated peer (client) over a TCP connection using
/// Tehuti's meeting, peer, and RPC primitives.
fn main() -> Result<(), Box<dyn Error>> {
    println!("Are you hosting a server? (y/n): ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let is_server = input.trim().to_lowercase() == "y";

    let factory = PeerFactory::default()
        .with_user_data(is_server)?
        .with_typed::<CounterRole>();

    tcp_example(is_server, ADDRESS, factory.into(), app)?;
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
        CounterRole::Authority { rpcs } => {
            replica_set.bind_rpc_sender(rpcs.clone());
        }
        CounterRole::Simulated { rpcs } => {
            replica_set.bind_rpc_receiver(rpcs.clone());
        }
    }

    // Create a counter replica.
    let mut counter = Counter {
        replica: replica_set.create(ReplicaId::new(0))?,
        value: 0,
    };

    let mut timer = Instant::now();
    loop {
        // Authority peer: increment the counter every second.
        if timer.elapsed() >= Duration::from_secs(1) {
            timer = Instant::now();

            if matches!(&peer, CounterRole::Authority { .. }) {
                counter.value += 1;

                println!("Tick counter: {}", counter.value);

                // Send an RPC to notify the simulated peer of the tick.
                if let Some(sender) = counter.replica.rpc_sender() {
                    sender.send(TickRpc::new("tick", counter.value))?;
                }
            }
        }

        // Simulated peer: receive tick RPCs and update the counter value.
        if let Some(Ok(decoder)) = counter.replica.rpc_receive()
            && decoder.procedure() == "tick"
        {
            let rpc = decoder.complete::<(), RepCodec<u32>>()?;
            let (_, value) = rpc.call()?;
            counter.value = value;
            println!("Counter ticked: {}", counter.value);
        }

        replica_set.maintain();

        sleep(Duration::from_millis(100));
    }
}

enum CounterRole {
    Authority {
        rpcs: Sender<Dispatch<ReplicationBuffer>>,
    },
    Simulated {
        rpcs: Receiver<Dispatch<ReplicationBuffer>>,
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
                RPC_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            ))
        } else {
            Ok(builder.bind_read::<ReplicationBuffer, ReplicationBuffer>(
                RPC_CHANNEL,
                ChannelMode::ReliableOrdered,
                None,
            ))
        }
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        let is_server = *peer.user_data().access::<bool>()?;

        if is_server {
            Ok(Self::Authority {
                rpcs: peer.write::<ReplicationBuffer>(RPC_CHANNEL)?,
            })
        } else {
            Ok(Self::Simulated {
                rpcs: peer.read::<ReplicationBuffer>(RPC_CHANNEL)?,
            })
        }
    }
}

struct Counter {
    replica: Replica,
    value: u32,
}
