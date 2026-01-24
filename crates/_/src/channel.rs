use crate::{Duplex, packet::PacketSerializer};
use flume::{Receiver, Sender};
use std::{
    any::{Any, TypeId},
    error::Error,
    hash::{DefaultHasher, Hash, Hasher},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChannelId(u64);

impl ChannelId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn hashed<T: Hash>(item: &T) -> Self {
        let mut hasher = DefaultHasher::new();
        item.hash(&mut hasher);
        Self(hasher.finish())
    }

    pub fn typed<T: 'static>() -> Self {
        Self::hashed(&TypeId::of::<T>())
    }

    pub fn id(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#channel:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChannelMode {
    ReliableOrdered,
    ReliableUnordered,
    Unreliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChannelKind {
    Read,
    Write,
    Pipe,
}

/// Type-erased channels used for converting between messages and bytes,
/// and forwarding them between user-side and backend-side.
/// Channels as a concept serve as a read/write communication layer of a peer
/// with outside world.
pub struct Channel {
    kind: ChannelKind,
    pump: Box<dyn FnMut() -> Result<bool, Box<dyn Error>> + Send>,
    pump_all: Box<dyn FnMut() -> Result<usize, Box<dyn Error>> + Send>,
}

impl Channel {
    pub fn read<Message: Send + 'static>(
        packet_receiver: Receiver<Vec<u8>>,
        message_sender: Sender<Message>,
        serializer: impl PacketSerializer<Message> + Send + 'static,
    ) -> Self {
        struct State<Message: Send + 'static> {
            packet_receiver: Receiver<Vec<u8>>,
            message_sender: Sender<Message>,
            serializer: Box<dyn PacketSerializer<Message> + Send>,
        }

        let state_pump = Arc::new(Mutex::new(State {
            packet_receiver,
            message_sender,
            serializer: Box::new(serializer),
        })) as Arc<Mutex<dyn Any + Send>>;
        let state_pump_all = state_pump.clone();
        let pump = Box::new(move || -> Result<bool, Box<dyn Error>> {
            let mut state = state_pump
                .lock()
                .map_err(|_| "Failed to lock read channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast read channel state")?;
            if let Ok(packet) = state.packet_receiver.try_recv() {
                let message = state.serializer.decode(&packet)?;
                state
                    .message_sender
                    .send(message)
                    .map_err(|err| format!("Pump message sender error: {err}"))?;
                Ok(true)
            } else {
                Ok(false)
            }
        });
        let pump_all = Box::new(move || -> Result<usize, Box<dyn Error>> {
            let mut count = 0;
            let mut state = state_pump_all
                .lock()
                .map_err(|_| "Failed to lock read channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast read channel state")?;
            for packet in state.packet_receiver.drain() {
                let message = state.serializer.decode(&packet)?;
                state
                    .message_sender
                    .send(message)
                    .map_err(|err| format!("Pump-all message sender error: {err}"))?;
                count += 1;
            }
            Ok(count)
        });
        Self {
            kind: ChannelKind::Read,
            pump,
            pump_all,
        }
    }

    pub fn write<Message: Send + 'static>(
        packet_sender: Sender<Vec<u8>>,
        message_receiver: Receiver<Message>,
        serializer: impl PacketSerializer<Message> + Send + 'static,
    ) -> Self {
        struct State<Message: Send + 'static> {
            packet_sender: Sender<Vec<u8>>,
            message_receiver: Receiver<Message>,
            serializer: Box<dyn PacketSerializer<Message> + Send>,
        }

        let state_pump = Arc::new(Mutex::new(State {
            packet_sender,
            message_receiver,
            serializer: Box::new(serializer),
        })) as Arc<Mutex<dyn Any + Send>>;
        let state_pump_all = state_pump.clone();
        let pump = Box::new(move || -> Result<bool, Box<dyn Error>> {
            let mut state = state_pump
                .lock()
                .map_err(|_| "Failed to lock write channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast write channel state")?;
            if let Ok(message) = state.message_receiver.try_recv() {
                let mut packet = Vec::new();
                state.serializer.encode(&message, &mut packet)?;
                state
                    .packet_sender
                    .send(packet)
                    .map_err(|err| format!("Pump packet sender error: {err}"))?;
                Ok(true)
            } else {
                Ok(false)
            }
        });
        let pump_all = Box::new(move || -> Result<usize, Box<dyn Error>> {
            let mut count = 0;
            let mut state = state_pump_all
                .lock()
                .map_err(|_| "Failed to lock write channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast write channel state")?;
            for message in state.message_receiver.drain() {
                let mut packet = Vec::new();
                state.serializer.encode(&message, &mut packet)?;
                state
                    .packet_sender
                    .send(packet)
                    .map_err(|err| format!("Pump-all packet sender error: {err}"))?;
                count += 1;
            }
            Ok(count)
        });
        Self {
            kind: ChannelKind::Write,
            pump,
            pump_all,
        }
    }

    pub fn pipe<Message: Send + 'static>(message: Duplex<Message>) -> Self {
        struct State<Message: Send + 'static> {
            message: Duplex<Message>,
        }

        let state_pump = Arc::new(Mutex::new(State { message })) as Arc<Mutex<dyn Any + Send>>;
        let state_pump_all = state_pump.clone();
        let pump = Box::new(move || -> Result<bool, Box<dyn Error>> {
            let mut state = state_pump
                .lock()
                .map_err(|_| "Failed to lock pipe channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast pipe channel state")?;
            if let Ok(message) = state.message.receiver.try_recv() {
                state
                    .message
                    .sender
                    .send(message)
                    .map_err(|err| format!("Pump pipe sender error: {err}"))?;
                Ok(true)
            } else {
                Ok(false)
            }
        });
        let pump_all = Box::new(move || -> Result<usize, Box<dyn Error>> {
            let mut count = 0;
            let mut state = state_pump_all
                .lock()
                .map_err(|_| "Failed to lock pipe channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast pipe channel state")?;
            for message in state.message.receiver.drain() {
                state
                    .message
                    .sender
                    .send(message)
                    .map_err(|err| format!("Pump-all pipe sender error: {err}"))?;
                count += 1;
            }
            Ok(count)
        });
        Self {
            kind: ChannelKind::Pipe,
            pump,
            pump_all,
        }
    }

    pub fn kind(&self) -> ChannelKind {
        self.kind
    }

    pub fn pump(&mut self) -> Result<bool, Box<dyn Error>> {
        (self.pump)()
    }

    pub fn pump_all(&mut self) -> Result<usize, Box<dyn Error>> {
        (self.pump_all)()
    }
}

impl Future for Channel {
    type Output = Result<(), Box<dyn Error>>;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.pump() {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => Poll::Pending,
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flume::unbounded;

    struct TestMessage {
        pub id: u32,
    }

    struct TestPacketSerializer;

    impl PacketSerializer<TestMessage> for TestPacketSerializer {
        fn encode(
            &mut self,
            message: &TestMessage,
            buffer: &mut Vec<u8>,
        ) -> Result<(), Box<dyn Error>> {
            buffer.extend_from_slice(&message.id.to_le_bytes());
            Ok(())
        }

        fn decode(&mut self, buffer: &[u8]) -> Result<TestMessage, Box<dyn Error>> {
            if buffer.len() != 4 {
                return Err("Invalid buffer length".into());
            }
            let id = u32::from_le_bytes(buffer.try_into().unwrap());
            Ok(TestMessage { id })
        }
    }

    #[test]
    fn test_async() {
        fn is_send<T: Send>() {}

        is_send::<Channel>();
    }

    #[test]
    fn test_channel_write() {
        let (msg_tx, msg_rx) = unbounded();
        let (pkt_tx, pkt_rx) = unbounded();

        let mut channel = Channel::write(pkt_tx, msg_rx, TestPacketSerializer);

        let message = TestMessage { id: 42 };
        msg_tx.send(message).unwrap();

        channel.pump().unwrap();

        let packet = pkt_rx.recv().unwrap();
        let message = TestPacketSerializer.decode(&packet).unwrap();
        assert_eq!(message.id, 42);
    }

    #[test]
    fn test_channel_read() {
        let (msg_tx, msg_rx) = unbounded();
        let (pkt_tx, pkt_rx) = unbounded();

        let mut channel = Channel::read(pkt_rx, msg_tx, TestPacketSerializer);

        let message = TestMessage { id: 42 };
        let mut packet = Vec::new();
        TestPacketSerializer.encode(&message, &mut packet).unwrap();
        pkt_tx.send(packet).unwrap();

        channel.pump().unwrap();

        let message = msg_rx.recv().unwrap();
        assert_eq!(message.id, 42);
    }

    #[test]
    fn test_channel_pipe() {
        let (comm, msg) = Duplex::crossing_unbounded();

        let mut channel = Channel::pipe(msg);

        let message = TestMessage { id: 42 };
        comm.sender.send(message).unwrap();

        channel.pump().unwrap();

        let message = comm.receiver.recv().unwrap();
        assert_eq!(message.id, 42);
    }
}
