use crate::{
    channel::{Channel, ChannelId, ChannelMode},
    engine::{EnginePacketReceiver, EnginePacketSender, EnginePeerDescriptor},
    meeting::MeetingUserEvent,
    packet::PacketSerializer,
};
use flume::{Receiver, Sender, unbounded};
use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    error::Error,
    hash::{DefaultHasher, Hash, Hasher},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(u64);

impl PeerId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn id(&self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#peer:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerRoleId(u64);

impl PeerRoleId {
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

impl std::fmt::Display for PeerRoleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#role:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub role_id: PeerRoleId,
    pub remote: bool,
}

/// User-side interface for a peer existing in a meeting.
/// Peer is constructed from peer role. It binds channels read/write interfaces
/// for communication on the user side.
#[derive(Debug)]
pub struct Peer {
    info: PeerInfo,
    receivers: BTreeMap<ChannelId, Box<dyn Any + Send>>,
    senders: BTreeMap<ChannelId, Box<dyn Any + Send>>,
    meeting_sender: Sender<MeetingUserEvent>,
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self
            .meeting_sender
            .send(MeetingUserEvent::PeerDestroy(self.info.peer_id));
    }
}

impl Peer {
    pub fn new(info: PeerInfo, meeting_sender: Sender<MeetingUserEvent>) -> Self {
        Self {
            info,
            receivers: Default::default(),
            senders: Default::default(),
            meeting_sender,
        }
    }

    pub fn info(&self) -> &PeerInfo {
        &self.info
    }

    pub fn send<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
        message: Message,
    ) -> Result<(), Box<dyn Error>> {
        self.senders
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Sender<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message sender {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?
            .send(message)
            .map_err(|err| format!("Peer message sender error: {err}"))?;
        Ok(())
    }

    pub async fn send_async<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
        message: Message,
    ) -> Result<(), Box<dyn Error>> {
        self.senders
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Sender<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message sender {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?
            .send_async(message)
            .await
            .map_err(|err| format!("Peer message sender error: {err}"))?;
        Ok(())
    }

    pub fn recv<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<Message, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        Ok(receiver.recv()?)
    }

    pub fn recv_timeout<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
        timeout: Duration,
    ) -> Result<Message, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        Ok(receiver.recv_timeout(timeout)?)
    }

    pub fn try_recv<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<Message, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        Ok(receiver.try_recv()?)
    }

    pub async fn recv_async<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<Message, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Message>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        let message = receiver.recv_async().await?;
        Ok(message)
    }
}

pub struct PeerBuildResult {
    pub peer: Peer,
    pub channels: Vec<Channel>,
    pub descriptor: EnginePeerDescriptor,
}

pub struct PeerBuilder {
    peer: Peer,
    read_channels: BTreeMap<ChannelId, (ChannelMode, Channel, Sender<Vec<u8>>)>,
    write_channels: BTreeMap<ChannelId, (ChannelMode, Channel, Receiver<Vec<u8>>)>,
}

impl PeerBuilder {
    pub fn new(
        peer_id: PeerId,
        role_id: PeerRoleId,
        remote: bool,
        meeting_sender: Sender<MeetingUserEvent>,
    ) -> Self {
        Self {
            peer: Peer::new(
                PeerInfo {
                    peer_id,
                    role_id,
                    remote,
                },
                meeting_sender,
            ),
            read_channels: Default::default(),
            write_channels: Default::default(),
        }
    }

    pub fn bind_read<Message: Send + 'static>(
        mut self,
        channel_id: ChannelId,
        mode: ChannelMode,
        serializer: impl PacketSerializer<Message> + Send + 'static,
    ) -> Self {
        let (packet_tx, packet_rx) = unbounded();
        let (message_tx, message_rx) = unbounded();
        let channel = Channel::read(packet_rx, message_tx, serializer);
        self.read_channels
            .insert(channel_id, (mode, channel, packet_tx));
        self.peer
            .receivers
            .insert(channel_id, Box::new(message_rx) as Box<dyn Any + Send>);
        self
    }

    pub fn bind_write<Message: Send + 'static>(
        mut self,
        channel_id: ChannelId,
        mode: ChannelMode,
        serializer: impl PacketSerializer<Message> + Send + 'static,
    ) -> Self {
        let (packet_tx, packet_rx) = unbounded();
        let (message_tx, message_rx) = unbounded();
        let channel = Channel::write(packet_tx, message_rx, serializer);
        self.write_channels
            .insert(channel_id, (mode, channel, packet_rx));
        self.peer
            .senders
            .insert(channel_id, Box::new(message_tx) as Box<dyn Any + Send>);
        self
    }

    pub fn build(self) -> PeerBuildResult {
        let mut channels = Vec::new();
        let mut packet_senders = BTreeMap::new();
        let mut packet_receivers = BTreeMap::new();
        for (channel_id, (mode, channel, packet_tx)) in self.read_channels {
            channels.push(channel);
            packet_senders.insert(
                channel_id,
                EnginePacketSender {
                    mode,
                    sender: packet_tx,
                },
            );
        }
        for (channel_id, (mode, channel, packet_rx)) in self.write_channels {
            channels.push(channel);
            packet_receivers.insert(
                channel_id,
                EnginePacketReceiver {
                    mode,
                    receiver: packet_rx,
                },
            );
        }
        let descriptor = EnginePeerDescriptor {
            info: self.peer.info,
            packet_senders,
            packet_receivers,
        };
        PeerBuildResult {
            peer: self.peer,
            channels,
            descriptor,
        }
    }
}

#[derive(Default)]
pub struct PeerFactory {
    registry: BTreeMap<PeerRoleId, Arc<dyn Fn(PeerBuilder) -> PeerBuilder + Send + Sync>>,
}

impl PeerFactory {
    pub fn with(
        mut self,
        role_id: PeerRoleId,
        builder_fn: impl Fn(PeerBuilder) -> PeerBuilder + Send + Sync + 'static,
    ) -> Self {
        self.register(role_id, builder_fn);
        self
    }

    pub fn register(
        &mut self,
        role_id: PeerRoleId,
        builder_fn: impl Fn(PeerBuilder) -> PeerBuilder + Send + Sync + 'static,
    ) {
        self.registry.insert(role_id, Arc::new(builder_fn));
    }

    pub fn create(
        &self,
        peer_id: PeerId,
        role_id: PeerRoleId,
        remote: bool,
        meeting_sender: Sender<MeetingUserEvent>,
    ) -> Result<PeerBuildResult, Box<dyn Error>> {
        let builder_fn = self
            .registry
            .get(&role_id)
            .ok_or_else(|| format!("No registered builder for role id {:?}", role_id))?;
        let builder = PeerBuilder::new(peer_id, role_id, remote, meeting_sender);
        let peer_builder = builder_fn(builder);
        Ok(peer_builder.build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPacketSerializer;

    impl PacketSerializer<u8> for TestPacketSerializer {
        fn encode(&mut self, message: &u8, buffer: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
            buffer.push(*message);
            Ok(())
        }

        fn decode(&mut self, buffer: &[u8]) -> Result<u8, Box<dyn Error>> {
            Ok(buffer[0])
        }
    }

    #[test]
    fn test_async() {
        fn is_send<T: Send>() {}

        is_send::<Peer>();
        is_send::<PeerBuildResult>();
        is_send::<PeerBuilder>();
        is_send::<PeerFactory>();
    }

    #[test]
    fn test_peer() {
        let factory = PeerFactory::default().with(PeerRoleId::new(0), |builder| {
            builder
                .bind_read::<u8>(
                    ChannelId::new(0),
                    ChannelMode::ReliableOrdered,
                    TestPacketSerializer,
                )
                .bind_write::<u8>(
                    ChannelId::new(0),
                    ChannelMode::ReliableOrdered,
                    TestPacketSerializer,
                )
        });
        let (meeting_tx, _) = unbounded();

        let peer_id = PeerId::new(0);
        let role_id = PeerRoleId::new(0);
        let PeerBuildResult {
            peer,
            mut channels,
            descriptor,
        } = factory.create(peer_id, role_id, false, meeting_tx).unwrap();

        assert_eq!(peer.info().peer_id, peer_id);
        assert_eq!(peer.info().role_id, role_id);
        assert_eq!(channels.len(), 2);

        peer.send(ChannelId::new(0), 42u8).unwrap();

        for channel in &mut channels {
            channel.pump_all().unwrap();
        }

        let packet = descriptor
            .packet_receivers
            .get(&ChannelId::new(0))
            .unwrap()
            .receiver
            .try_recv()
            .unwrap();
        let message = TestPacketSerializer.decode(&packet).unwrap();
        assert_eq!(message, 42u8);

        let mut packet = Vec::new();
        TestPacketSerializer.encode(&100, &mut packet).unwrap();
        descriptor
            .packet_senders
            .get(&ChannelId::new(0))
            .unwrap()
            .sender
            .send(packet)
            .unwrap();

        for channel in &mut channels {
            channel.pump_all().unwrap();
        }

        let received_message = peer.try_recv::<u8>(ChannelId::new(0)).unwrap();
        assert_eq!(received_message, 100u8);
    }
}
