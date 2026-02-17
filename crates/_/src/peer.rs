use crate::{
    buffer::Buffer,
    channel::{Channel, ChannelId, ChannelMode, Dispatch},
    codec::Codec,
    engine::{EnginePacketReceiver, EnginePacketSender, EnginePeerDescriptor},
    event::{Duplex, Receiver, Sender, bounded, unbounded},
    meeting::MeetingUserEvent,
    protocol::ProtocolPacketData,
    replication::Replicable,
};
use serde::{Deserialize, Serialize};
use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, HashMap},
    error::Error,
    hash::Hash,
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerId(u64);

impl PeerId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> u64 {
        self.0
    }
}

impl Replicable for PeerId {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#peer:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerRoleId(u64);

impl PeerRoleId {
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

impl Replicable for PeerRoleId {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for PeerRoleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#role:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub role_id: PeerRoleId,
    pub remote: bool,
}

#[derive(Default, Clone)]
pub struct PeerUserData(Arc<HashMap<TypeId, Box<dyn Any + Send + Sync>>>);

impl PeerUserData {
    pub fn is_frozen(&self) -> bool {
        Arc::strong_count(&self.0) > 1
    }

    pub fn with<T: Any + Send + Sync + 'static>(mut self, data: T) -> Result<Self, Box<dyn Error>> {
        Arc::get_mut(&mut self.0)
            .ok_or("PeerUserData is frozen")?
            .insert(TypeId::of::<T>(), Box::new(data));
        Ok(self)
    }

    pub fn access<T: Any + Send + Sync + 'static>(&self) -> Result<&T, Box<dyn Error>> {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|data| data.downcast_ref::<T>())
            .ok_or_else(|| {
                format!(
                    "User data for type: {} not found!",
                    std::any::type_name::<T>()
                )
                .into()
            })
    }
}

impl std::fmt::Debug for PeerUserData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerUserData").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct PeerKiller {
    peer_id: PeerId,
    meeting_sender: Sender<MeetingUserEvent>,
    pub destroy_on_drop: bool,
}

impl PeerKiller {
    pub fn new(peer_id: PeerId, meeting_sender: Sender<MeetingUserEvent>) -> Self {
        Self {
            peer_id,
            meeting_sender,
            destroy_on_drop: true,
        }
    }
}

impl Drop for PeerKiller {
    fn drop(&mut self) {
        if self.destroy_on_drop {
            let _ = self
                .meeting_sender
                .send(MeetingUserEvent::PeerDestroy(self.peer_id));
        }
    }
}

/// User-side interface for a peer existing in a meeting.
/// Peer is constructed from peer role. It binds channels read/write interfaces
/// for communication on the user side.
#[derive(Debug)]
pub struct Peer {
    info: PeerInfo,
    user_data: PeerUserData,
    receivers: BTreeMap<ChannelId, Box<dyn Any + Send>>,
    senders: BTreeMap<ChannelId, Box<dyn Any + Send>>,
    killer: PeerKiller,
}

impl Peer {
    pub fn new(info: PeerInfo, meeting_sender: Sender<MeetingUserEvent>) -> Self {
        Self {
            info,
            user_data: Default::default(),
            receivers: Default::default(),
            senders: Default::default(),
            killer: PeerKiller::new(info.peer_id, meeting_sender),
        }
    }

    pub fn with_user_data(mut self, data: PeerUserData) -> Self {
        self.user_data = data;
        self
    }

    pub fn user_data(&self) -> &PeerUserData {
        &self.user_data
    }

    pub fn destructure(self) -> PeerDestructurer {
        PeerDestructurer::new(self)
    }

    pub fn into_typed<T: TypedPeerRole>(self) -> Result<T, Box<dyn Error>> {
        T::into_typed(self.destructure())
    }

    pub fn info(&self) -> &PeerInfo {
        &self.info
    }

    pub fn sender<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<&Sender<Dispatch<Message>>, Box<dyn Error>> {
        self.senders
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Sender<Dispatch<Message>>>()
            .ok_or_else(|| {
                format!(
                    "Message sender {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
                .into()
            })
    }

    pub fn receiver<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<&Receiver<Dispatch<Message>>, Box<dyn Error>> {
        self.receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Dispatch<Message>>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
                .into()
            })
    }

    pub fn send<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
        message: Dispatch<Message>,
    ) -> Result<(), Box<dyn Error>> {
        self.senders
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Sender<Dispatch<Message>>>()
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
        message: Dispatch<Message>,
    ) -> Result<(), Box<dyn Error>> {
        self.senders
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Sender<Dispatch<Message>>>()
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
    ) -> Result<Option<Dispatch<Message>>, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Dispatch<Message>>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        receiver.recv()
    }

    pub fn recv_blocking<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<Dispatch<Message>, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Dispatch<Message>>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        receiver.recv_blocking()
    }

    pub fn recv_blocking_timeout<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
        duration: Duration,
    ) -> Result<Dispatch<Message>, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Dispatch<Message>>>()
            .ok_or_else(|| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.info.peer_id
                )
            })?;
        receiver.recv_blocking_timeout(duration)
    }

    pub async fn recv_async<Message: Send + 'static>(
        &self,
        channel_id: ChannelId,
    ) -> Result<Dispatch<Message>, Box<dyn Error>> {
        let receiver = self
            .receivers
            .get(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.info.peer_id, channel_id
                )
            })?
            .downcast_ref::<Receiver<Dispatch<Message>>>()
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

struct PeerBuilderChannel {
    mode: ChannelMode,
    channel: Channel,
}

pub struct PeerBuilder {
    peer: Peer,
    read_channels: BTreeMap<ChannelId, (PeerBuilderChannel, Sender<ProtocolPacketData>)>,
    write_channels: BTreeMap<ChannelId, (PeerBuilderChannel, Receiver<ProtocolPacketData>)>,
}

impl PeerBuilder {
    pub fn new(peer: Peer) -> Self {
        Self {
            peer,
            read_channels: Default::default(),
            write_channels: Default::default(),
        }
    }

    pub fn info(&self) -> &PeerInfo {
        &self.peer.info
    }

    pub fn user_data(&self) -> &PeerUserData {
        &self.peer.user_data
    }

    pub fn bind_read<C: Codec<Value = Message> + Send + 'static, Message: Send + 'static>(
        mut self,
        channel_id: ChannelId,
        mode: ChannelMode,
        capacity: Option<usize>,
    ) -> Self {
        let (packet_tx, packet_rx) = if let Some(capacity) = capacity {
            bounded(capacity)
        } else {
            unbounded()
        };
        let (message_tx, message_rx) = if let Some(capacity) = capacity {
            bounded(capacity)
        } else {
            unbounded()
        };
        let channel = Channel::read::<C, Message>(packet_rx, message_tx);
        self.read_channels.insert(
            channel_id,
            (PeerBuilderChannel { mode, channel }, packet_tx),
        );
        self.peer
            .receivers
            .insert(channel_id, Box::new(message_rx) as Box<dyn Any + Send>);
        self
    }

    pub fn bind_write<C: Codec<Value = Message> + Send + 'static, Message: Send + 'static>(
        mut self,
        channel_id: ChannelId,
        mode: ChannelMode,
        capacity: Option<usize>,
    ) -> Self {
        let (packet_tx, packet_rx) = if let Some(capacity) = capacity {
            bounded(capacity)
        } else {
            unbounded()
        };
        let (message_tx, message_rx) = if let Some(capacity) = capacity {
            bounded(capacity)
        } else {
            unbounded()
        };
        let channel = Channel::write::<C, Message>(packet_tx, message_rx);
        self.write_channels.insert(
            channel_id,
            (PeerBuilderChannel { mode, channel }, packet_rx),
        );
        self.peer
            .senders
            .insert(channel_id, Box::new(message_tx) as Box<dyn Any + Send>);
        self
    }

    pub fn bind_read_write<C: Codec<Value = Message> + Send + 'static, Message: Send + 'static>(
        self,
        channel_id: ChannelId,
        mode: ChannelMode,
        capacity: Option<usize>,
    ) -> Self {
        self.bind_read::<C, Message>(channel_id, mode, capacity)
            .bind_write::<C, Message>(channel_id, mode, capacity)
    }

    pub fn build(self) -> PeerBuildResult {
        let mut channels = Vec::new();
        let mut packet_senders = BTreeMap::new();
        let mut packet_receivers = BTreeMap::new();
        for (channel_id, (PeerBuilderChannel { mode, channel }, packet_tx)) in self.read_channels {
            channels.push(channel);
            packet_senders.insert(
                channel_id,
                EnginePacketSender {
                    mode,
                    sender: packet_tx,
                },
            );
        }
        for (channel_id, (PeerBuilderChannel { mode, channel }, packet_rx)) in self.write_channels {
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

pub trait TypedPeerRole: TypedPeer {
    const ROLE_ID: PeerRoleId;
}

pub trait TypedPeer: Sized {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        Ok(builder)
    }

    #[allow(unused_variables)]
    fn into_typed(peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        Err(format!(
            "{}::into_typed() not implemented",
            std::any::type_name::<Self>()
        )
        .into())
    }
}

impl TypedPeer for () {
    fn into_typed(_peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        Ok(())
    }
}

pub struct PeerDestructurer {
    peer: Peer,
}

impl PeerDestructurer {
    pub fn new(mut peer: Peer) -> Self {
        peer.killer.destroy_on_drop = false;
        Self { peer }
    }

    pub fn into_peer(self) -> Peer {
        self.peer
    }

    pub fn info(&self) -> &PeerInfo {
        self.peer.info()
    }

    pub fn user_data(&self) -> &PeerUserData {
        self.peer.user_data()
    }

    pub fn killer(&mut self) -> PeerKiller {
        PeerKiller {
            peer_id: self.peer.info.peer_id,
            meeting_sender: self.peer.killer.meeting_sender.clone(),
            destroy_on_drop: true,
        }
    }

    pub fn read<Message: Send + 'static>(
        &mut self,
        channel_id: ChannelId,
    ) -> Result<Receiver<Dispatch<Message>>, Box<dyn Error>> {
        let receiver = self
            .peer
            .receivers
            .remove(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message receiver with id {:?}",
                    self.peer.info.peer_id, channel_id
                )
            })?
            .downcast::<Receiver<Dispatch<Message>>>()
            .map_err(|_| {
                format!(
                    "Message receiver {:?} for Peer {:?} has different message type",
                    channel_id, self.peer.info.peer_id
                )
            })?;
        Ok(*receiver)
    }

    pub fn write<Message: Send + 'static>(
        &mut self,
        channel_id: ChannelId,
    ) -> Result<Sender<Dispatch<Message>>, Box<dyn Error>> {
        let sender = self
            .peer
            .senders
            .remove(&channel_id)
            .ok_or_else(|| {
                format!(
                    "Peer {:?} has no message sender with id {:?}",
                    self.peer.info.peer_id, channel_id
                )
            })?
            .downcast::<Sender<Dispatch<Message>>>()
            .map_err(|_| {
                format!(
                    "Message sender {:?} for Peer {:?} has different message type",
                    channel_id, self.peer.info.peer_id
                )
            })?;
        Ok(*sender)
    }

    pub fn read_write<Message: Send + 'static>(
        &mut self,
        channel_id: ChannelId,
    ) -> Result<Duplex<Dispatch<Message>>, Box<dyn Error>> {
        let receiver = self.read::<Message>(channel_id)?;
        let sender = self.write::<Message>(channel_id)?;
        Ok(Duplex { sender, receiver })
    }
}

#[derive(Default)]
pub struct PeerFactory {
    #[allow(clippy::type_complexity)]
    registry: BTreeMap<
        PeerRoleId,
        Arc<dyn Fn(PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> + Send + Sync>,
    >,
    user_data: PeerUserData,
}

impl PeerFactory {
    pub fn new(mut self, user_data: PeerUserData) -> Self {
        self.user_data = user_data;
        self
    }

    pub fn with_user_data<T: Any + Send + Sync + 'static>(
        mut self,
        data: T,
    ) -> Result<Self, Box<dyn Error>> {
        self.user_data = self.user_data.with(data)?;
        Ok(self)
    }

    pub fn with(
        mut self,
        role_id: PeerRoleId,
        builder_fn: impl Fn(PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> + Send + Sync + 'static,
    ) -> Self {
        self.register(role_id, builder_fn);
        self
    }

    pub fn with_typed<T: TypedPeerRole + 'static>(mut self) -> Self {
        self.register_typed::<T>();
        self
    }

    pub fn register(
        &mut self,
        role_id: PeerRoleId,
        builder_fn: impl Fn(PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> + Send + Sync + 'static,
    ) {
        tracing::event!(
            target: "tehuti::peer",
            tracing::Level::DEBUG,
            "Registering peer role id {}", role_id
        );
        self.registry.insert(role_id, Arc::new(builder_fn));
    }

    pub fn register_typed<T: TypedPeerRole + 'static>(&mut self) {
        self.register(T::ROLE_ID, T::builder);
    }

    pub fn create(&self, peer: Peer) -> Result<PeerBuildResult, Box<dyn Error>> {
        let builder_fn = self
            .registry
            .get(&peer.info.role_id)
            .ok_or_else(|| format!("No registered builder for role id {:?}", peer.info.role_id))?;
        let builder = PeerBuilder::new(peer.with_user_data(self.user_data.clone()));
        Ok(builder_fn(builder)?.build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::replicable::RepCodec;
    use std::io::Cursor;

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
            Ok(builder.bind_read_write::<RepCodec<u8>, u8>(
                ChannelId::new(0),
                ChannelMode::ReliableOrdered,
                None,
            ))
        });
        let (meeting_tx, _) = unbounded();

        let peer_id = PeerId::new(0);
        let role_id = PeerRoleId::new(0);
        let PeerBuildResult {
            peer,
            mut channels,
            descriptor,
        } = factory
            .create(Peer::new(
                PeerInfo {
                    peer_id,
                    role_id,
                    remote: false,
                },
                meeting_tx,
            ))
            .unwrap();

        assert_eq!(peer.info().peer_id, peer_id);
        assert_eq!(peer.info().role_id, role_id);
        assert_eq!(channels.len(), 2);

        let events = peer
            .destructure()
            .read_write::<u8>(ChannelId::new(0))
            .unwrap();

        events.sender.send(42u8.into()).unwrap();

        for channel in &mut channels {
            channel.pump_all().unwrap();
        }

        let packet = descriptor
            .packet_receivers
            .get(&ChannelId::new(0))
            .unwrap()
            .receiver
            .recv_blocking()
            .unwrap();
        let mut buffer = Cursor::new(packet.data);
        let message = RepCodec::<u8>::decode(&mut buffer).unwrap();
        assert_eq!(message, 42u8);

        let mut buffer = Cursor::new(Vec::new());
        RepCodec::<u8>::encode(&100, &mut buffer).unwrap();
        descriptor
            .packet_senders
            .get(&ChannelId::new(0))
            .unwrap()
            .sender
            .send(ProtocolPacketData {
                data: buffer.into_inner(),
                recepients: Default::default(),
            })
            .unwrap();

        for channel in &mut channels {
            channel.pump_all().unwrap();
        }

        let received_message = events.receiver.recv_blocking().unwrap();
        assert_eq!(received_message.message, 100u8);
    }
}
