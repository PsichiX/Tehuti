//! Tehuti is an opinionated communication library, giving useful pluggable
//! primitives for building custom communication systems for various use-cases.
//!
//! Key concepts:
//! - Peers: Individual participants in the communication system.
//! - Meetings: Place where peers gather together to communicate.
//! - Channels: Communication pathways between peers in a meeting.
//! - Packets: The raw data being transmitted between peers over channels.
//! - Protocols: Low-level rules and conventions for communication in a meeting.
//! - Engines: Custom backends that handle meetings communication in a specific way.
//!
//! How typical communication works:
//! 1. Engine on each machine creates a Meeting - in case of networking, think
//!    of it as a session, where one machine can host and other machines join.
//! 1. Meeting creates local peer(s) - think of them as actors that can replicate
//!    their state and behavior across machines. Peers define and bind to specific
//!    channels with read/write rights. These define a peer role in the meeting.
//!    For example instead hardcoded client/server roles, you define which peer
//!    role has rights to read/write on which channels - server could have write
//!    rights to all channels, since it is the source of truth, while clients
//!    could have read rights to most channels, but write rights only to a few
//!    channels (e.g. their own input state).
//! 1. Peers get replicated on all machines joining the meeting, so each machine
//!    has a local representation of all remote peers. These remote peers are
//!    automatically kept in sync by the meeting engine, which uses the defined
//!    protocols to replicate peer state and behavior across machines.
//! 1. You send messages (packets) between peers over their bound channels.
//!    Messages get converted to packets, transmitted over the network (or
//!    other medium), received on the other side, converted back to messages,
//!    and delivered to all machines peer(s).

pub mod channel;
pub mod codec;
pub mod engine;
pub mod meeting;
pub mod peer;
pub mod protocol;
pub mod replication;

pub mod third_party {
    pub use flume;
    pub use tracing;
}

use flume::{Receiver, Sender, bounded, unbounded};

#[derive(Debug, Clone)]
pub struct Duplex<T> {
    pub sender: Sender<T>,
    pub receiver: Receiver<T>,
}

impl<T> Duplex<T> {
    pub fn unbounded() -> Self {
        let (tx, rx) = unbounded();
        Self {
            sender: tx,
            receiver: rx,
        }
    }

    pub fn bounded(capacity: usize) -> Self {
        let (tx, rx) = bounded(capacity);
        Self {
            sender: tx,
            receiver: rx,
        }
    }

    pub fn crossing_unbounded() -> (Self, Self) {
        let (tx1, rx1) = unbounded();
        let (tx2, rx2) = unbounded();
        (
            Self {
                sender: tx1,
                receiver: rx2,
            },
            Self {
                sender: tx2,
                receiver: rx1,
            },
        )
    }

    pub fn crossing_bounded(capacity: usize) -> (Self, Self) {
        let (tx1, rx1) = bounded(capacity);
        let (tx2, rx2) = bounded(capacity);
        (
            Self {
                sender: tx1,
                receiver: rx2,
            },
            Self {
                sender: tx2,
                receiver: rx1,
            },
        )
    }

    pub fn new(sender: Sender<T>, receiver: Receiver<T>) -> Self {
        Self { sender, receiver }
    }
}
