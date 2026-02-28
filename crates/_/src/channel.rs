use crate::{
    buffer::Buffer,
    codec::Codec,
    engine::EngineId,
    event::{Receiver, Sender},
    protocol::{PacketRecepients, ProtocolPacketData},
    replication::Replicable,
};
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    error::Error,
    hash::Hash,
    io::Cursor,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ChannelId(u64);

impl ChannelId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn hashed<T: Hash>(item: &T) -> Self {
        Self(crate::hash(item))
    }

    pub fn typed<T: 'static>() -> Self {
        Self::hashed(&std::any::type_name::<T>())
    }

    pub const fn id(&self) -> u64 {
        self.0
    }
}

impl Replicable for ChannelId {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for ChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#channel:{}", self.0)
    }
}

pub struct Dispatch<Message: Send + 'static> {
    pub message: Message,
    pub recepients: PacketRecepients,
    pub sender: Option<EngineId>,
}

impl<Message: Send + 'static> Dispatch<Message> {
    pub fn new(message: Message) -> Self {
        Self {
            message,
            recepients: Default::default(),
            sender: None,
        }
    }

    pub fn recepient(mut self, engine_id: EngineId) -> Self {
        self.recepients.push(engine_id);
        self
    }

    pub fn maybe_recepient(mut self, engine_id: Option<EngineId>) -> Self {
        if let Some(engine_id) = engine_id {
            self.recepients.push(engine_id);
        }
        self
    }

    pub fn recepients(mut self, engine_ids: impl IntoIterator<Item = EngineId>) -> Self {
        self.recepients.extend(engine_ids);
        self
    }

    pub fn sender(mut self, engine_id: EngineId) -> Self {
        self.sender = Some(engine_id);
        self
    }

    pub fn maybe_sender(mut self, engine_id: Option<EngineId>) -> Self {
        self.sender = engine_id;
        self
    }
}

impl<Message: Send + 'static> From<Message> for Dispatch<Message> {
    fn from(message: Message) -> Self {
        Self::new(message)
    }
}

/// Mode of the channel, defining its reliability and ordering guarantees.
/// IMPORTANT: It is only a hint for the underlying transport, actual guarantees
/// depend on the transport used!
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChannelMode {
    ReliableOrdered,
    ReliableUnordered,
    Unreliable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ChannelKind {
    Read,
    Write,
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
    pub fn read<C: Codec<Value = Message>, Message: Send + 'static>(
        packet_receiver: Receiver<ProtocolPacketData>,
        message_sender: Sender<Dispatch<Message>>,
    ) -> Self {
        struct State<Message: Send + 'static> {
            packet_receiver: Receiver<ProtocolPacketData>,
            message_sender: Sender<Dispatch<Message>>,
        }

        let state_pump = Arc::new(Mutex::new(State {
            packet_receiver,
            message_sender,
        })) as Arc<Mutex<dyn Any + Send>>;
        let state_pump_all = state_pump.clone();
        let pump = Box::new(move || -> Result<bool, Box<dyn Error>> {
            let mut state = state_pump
                .lock()
                .map_err(|_| "Failed to lock read channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast read channel state")?;
            if let Some(ProtocolPacketData {
                data,
                recepients,
                sender,
            }) = state.packet_receiver.try_recv()
            {
                let mut buffer = Cursor::new(data);
                let message = C::decode(&mut buffer)?;
                state
                    .message_sender
                    .send(Dispatch {
                        message,
                        recepients,
                        sender,
                    })
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
            for ProtocolPacketData {
                data,
                recepients,
                sender,
            } in state.packet_receiver.iter()
            {
                let mut buffer = Cursor::new(data);
                let message = C::decode(&mut buffer)?;
                state
                    .message_sender
                    .send(Dispatch {
                        message,
                        recepients,
                        sender,
                    })
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

    pub fn write<C: Codec<Value = Message>, Message: Send + 'static>(
        packet_sender: Sender<ProtocolPacketData>,
        message_receiver: Receiver<Dispatch<Message>>,
    ) -> Self {
        struct State<Message: Send + 'static> {
            packet_sender: Sender<ProtocolPacketData>,
            message_receiver: Receiver<Dispatch<Message>>,
        }

        let state_pump = Arc::new(Mutex::new(State {
            packet_sender,
            message_receiver,
        })) as Arc<Mutex<dyn Any + Send>>;
        let state_pump_all = state_pump.clone();
        let pump = Box::new(move || -> Result<bool, Box<dyn Error>> {
            let mut state = state_pump
                .lock()
                .map_err(|_| "Failed to lock write channel state")?;
            let state = state
                .downcast_mut::<State<Message>>()
                .ok_or("Failed to downcast write channel state")?;
            if let Some(Dispatch {
                message,
                recepients,
                sender,
            }) = state.message_receiver.try_recv()
            {
                let mut buffer = Cursor::new(Vec::new());
                C::encode(&message, &mut buffer)?;
                state
                    .packet_sender
                    .send(ProtocolPacketData {
                        data: buffer.into_inner(),
                        recepients,
                        sender,
                    })
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
            for Dispatch {
                message,
                recepients,
                sender,
            } in state.message_receiver.iter()
            {
                let mut buffer = Cursor::new(Vec::new());
                C::encode(&message, &mut buffer)?;
                state
                    .packet_sender
                    .send(ProtocolPacketData {
                        data: buffer.into_inner(),
                        recepients,
                        sender,
                    })
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

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.pump() {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{codec::postcard::PostcardCodec, event::unbounded};
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct TestMessage {
        pub id: u32,
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

        let mut channel = Channel::write::<PostcardCodec<TestMessage>, _>(pkt_tx, msg_rx);

        let message = TestMessage { id: 42 };
        msg_tx.send(message.into()).unwrap();

        channel.pump().unwrap();

        let packet = pkt_rx.recv_blocking().unwrap();
        let mut buffer = Cursor::new(packet.data);
        let message = PostcardCodec::<TestMessage>::decode(&mut buffer).unwrap();
        assert_eq!(message.id, 42);
    }

    #[test]
    fn test_channel_read() {
        let (msg_tx, msg_rx) = unbounded();
        let (pkt_tx, pkt_rx) = unbounded();

        let mut channel = Channel::read::<PostcardCodec<TestMessage>, _>(pkt_rx, msg_tx);

        let message = TestMessage { id: 42 };
        let mut buffer = Cursor::new(Vec::new());
        PostcardCodec::<TestMessage>::encode(&message, &mut buffer).unwrap();
        pkt_tx
            .send(ProtocolPacketData {
                data: buffer.into_inner(),
                recepients: Default::default(),
                sender: None,
            })
            .unwrap();

        channel.pump().unwrap();

        let message = msg_rx.recv_blocking().unwrap();
        assert_eq!(message.message.id, 42);
    }
}
