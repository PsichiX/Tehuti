use crate::{
    channel::{ChannelId, Dispatch},
    codec::Codec,
    engine::EngineId,
    event::{Duplex, Receiver, Sender, unbounded},
    peer::Peer,
    protocol::{PacketRecepients, ProtocolPacketData},
    replication::{
        BufferRead, BufferWrite, Replicable, Replicated, ReplicationPolicy,
        rpc::{Rpc, RpcPartialDecoder},
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    error::Error,
    io::{Cursor, Read, Write},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ReplicaId(u64);

impl ReplicaId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> u64 {
        self.0
    }
}

impl Replicable for ReplicaId {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#replica:{}", self.0)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplicationBuffer {
    replica_id: ReplicaId,
    buffer: Vec<u8>,
}

impl ReplicationBuffer {
    pub fn replica_id(&self) -> ReplicaId {
        self.replica_id
    }
}

impl Codec for ReplicationBuffer {
    type Value = Self;

    fn encode(message: &Self::Value, buffer: &mut dyn Write) -> Result<(), Box<dyn Error>> {
        buffer.write_all(&message.replica_id.id().to_le_bytes())?;
        buffer.write_all(&(message.buffer.len() as u64).to_le_bytes())?;
        buffer.write_all(&message.buffer)?;
        Ok(())
    }

    fn decode(buffer: &mut dyn Read) -> Result<Self::Value, Box<dyn Error>> {
        let mut replica_id_bytes = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut replica_id_bytes)?;
        let replica_id = ReplicaId::new(u64::from_le_bytes(replica_id_bytes));
        let mut len_bytes = [0u8; std::mem::size_of::<u64>()];
        buffer.read_exact(&mut len_bytes)?;
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut data = vec![0u8; len];
        buffer.read_exact(&mut data)?;
        Ok(Self {
            replica_id,
            buffer: data,
        })
    }
}

struct ReplicaInstance {
    change_sender: Option<Sender<ProtocolPacketData>>,
    change_receiver: Option<Receiver<ProtocolPacketData>>,
    rpc_sender: Option<Sender<ProtocolPacketData>>,
    rpc_receiver: Option<Receiver<ProtocolPacketData>>,
}

pub struct ReplicaSet {
    change_sender: Option<Sender<Dispatch<ReplicationBuffer>>>,
    change_receiver: Option<Receiver<Dispatch<ReplicationBuffer>>>,
    rpc_sender: Option<Sender<Dispatch<ReplicationBuffer>>>,
    rpc_receiver: Option<Receiver<Dispatch<ReplicationBuffer>>>,
    instances: BTreeMap<ReplicaId, ReplicaInstance>,
    killed_instances: Duplex<ReplicaId>,
}

impl Default for ReplicaSet {
    fn default() -> Self {
        Self {
            change_sender: None,
            change_receiver: None,
            rpc_sender: None,
            rpc_receiver: None,
            instances: Default::default(),
            killed_instances: Duplex::unbounded(),
        }
    }
}

impl ReplicaSet {
    pub fn create(&mut self, id: ReplicaId) -> Result<Replica, Box<dyn Error>> {
        if self.instances.contains_key(&id) {
            return Err(format!("Replica with ID {} already exists", id).into());
        }
        let (instance_change_sender, set_change_receiver) = unbounded();
        let (set_change_sender, instance_change_receiver) = unbounded();
        let (instance_rpc_sender, set_rpc_receiver) = unbounded();
        let (set_rpc_sender, instance_rpc_receiver) = unbounded();
        self.instances.insert(
            id,
            ReplicaInstance {
                change_sender: Some(set_change_sender),
                change_receiver: Some(set_change_receiver),
                rpc_sender: Some(set_rpc_sender),
                rpc_receiver: Some(set_rpc_receiver),
            },
        );
        Ok(Replica {
            id,
            change_sender: Some(instance_change_sender),
            change_receiver: Some(instance_change_receiver),
            rpc_sender: Some(instance_rpc_sender),
            rpc_receiver: Some(instance_rpc_receiver),
            killed_sender: self.killed_instances.sender.clone(),
        })
    }

    pub fn destroy(&mut self, id: &ReplicaId) -> bool {
        self.instances.remove(id).is_some()
    }

    pub fn has(&self, id: &ReplicaId) -> bool {
        self.instances.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = ReplicaId> {
        self.instances.keys().copied()
    }

    pub fn maintain(&mut self) {
        for id in self.killed_instances.receiver.iter().collect::<Vec<_>>() {
            self.destroy(&id);
        }
        if let Some(change_sender) = &self.change_sender {
            for (replica_id, instance) in &self.instances {
                if let Some(receiver) = &instance.change_receiver {
                    for ProtocolPacketData { data, recepients } in receiver.iter() {
                        let _ = change_sender.send(Dispatch {
                            message: ReplicationBuffer {
                                replica_id: *replica_id,
                                buffer: data,
                            },
                            recepients,
                        });
                    }
                }
            }
        }
        if let Some(change_receiver) = &self.change_receiver {
            for Dispatch {
                message,
                recepients,
            } in change_receiver.iter()
            {
                if let Some(instance) = self.instances.get_mut(&message.replica_id)
                    && let Some(sender) = &instance.change_sender
                {
                    let _ = sender.send(ProtocolPacketData {
                        data: message.buffer,
                        recepients,
                    });
                }
            }
        }
        if let Some(rpc_sender) = &self.rpc_sender {
            for (replica_id, instance) in &self.instances {
                if let Some(receiver) = &instance.rpc_receiver {
                    for ProtocolPacketData { data, recepients } in receiver.iter() {
                        let _ = rpc_sender.send(Dispatch {
                            message: ReplicationBuffer {
                                replica_id: *replica_id,
                                buffer: data,
                            },
                            recepients,
                        });
                    }
                }
            }
        }
        if let Some(rpc_receiver) = &self.rpc_receiver {
            for Dispatch {
                message,
                recepients,
            } in rpc_receiver.iter()
            {
                if let Some(instance) = self.instances.get_mut(&message.replica_id)
                    && let Some(sender) = &instance.rpc_sender
                {
                    let _ = sender.send(ProtocolPacketData {
                        data: message.buffer,
                        recepients,
                    });
                }
            }
        }
    }

    pub fn bind_peer(
        &mut self,
        peer: &Peer,
        change_channel_id: Option<ChannelId>,
        rpc_channel_id: Option<ChannelId>,
    ) {
        if let Some(change_channel_id) = change_channel_id {
            self.change_sender = peer
                .sender::<ReplicationBuffer>(change_channel_id)
                .ok()
                .cloned();
            self.change_receiver = peer
                .receiver::<ReplicationBuffer>(change_channel_id)
                .ok()
                .cloned();
        }
        if let Some(rpc_channel_id) = rpc_channel_id {
            self.rpc_sender = peer
                .sender::<ReplicationBuffer>(rpc_channel_id)
                .ok()
                .cloned();
            self.rpc_receiver = peer
                .receiver::<ReplicationBuffer>(rpc_channel_id)
                .ok()
                .cloned();
        }
    }

    pub fn bind_change(&mut self, change: Duplex<Dispatch<ReplicationBuffer>>) {
        self.change_sender = Some(change.sender);
        self.change_receiver = Some(change.receiver);
    }

    pub fn bind_rpc(&mut self, rpc: Duplex<Dispatch<ReplicationBuffer>>) {
        self.rpc_sender = Some(rpc.sender);
        self.rpc_receiver = Some(rpc.receiver);
    }

    pub fn bind_change_sender(&mut self, change_sender: Sender<Dispatch<ReplicationBuffer>>) {
        self.change_sender = Some(change_sender);
    }

    pub fn bind_change_receiver(&mut self, change_receiver: Receiver<Dispatch<ReplicationBuffer>>) {
        self.change_receiver = Some(change_receiver);
    }

    pub fn bind_rpc_sender(&mut self, rpc_sender: Sender<Dispatch<ReplicationBuffer>>) {
        self.rpc_sender = Some(rpc_sender);
    }

    pub fn bind_rpc_receiver(&mut self, rpc_receiver: Receiver<Dispatch<ReplicationBuffer>>) {
        self.rpc_receiver = Some(rpc_receiver);
    }

    pub fn unbind(&mut self) {
        self.change_sender = None;
        self.change_receiver = None;
        self.rpc_sender = None;
        self.rpc_receiver = None;
    }
}

pub struct ReplicaRpcSender {
    sender: Sender<ProtocolPacketData>,
    recepients: PacketRecepients,
}

impl ReplicaRpcSender {
    pub fn recepient(mut self, engine_id: EngineId) -> Self {
        self.recepients.push(engine_id);
        self
    }

    pub fn recepients(mut self, engine_ids: impl IntoIterator<Item = EngineId>) -> Self {
        self.recepients.extend(engine_ids);
        self
    }

    pub fn send<Output, Input>(&self, rpc: Rpc<Output, Input>) -> Result<(), Box<dyn Error>>
    where
        Output: Codec + Sized,
        Input: Codec + Sized,
    {
        let mut buffer: Vec<u8> = Vec::new();
        rpc.encode(&mut buffer)?;
        self.sender.send(ProtocolPacketData {
            data: buffer,
            recepients: self.recepients.clone(),
        })?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct Replica {
    id: ReplicaId,
    change_sender: Option<Sender<ProtocolPacketData>>,
    change_receiver: Option<Receiver<ProtocolPacketData>>,
    rpc_sender: Option<Sender<ProtocolPacketData>>,
    rpc_receiver: Option<Receiver<ProtocolPacketData>>,
    killed_sender: Sender<ReplicaId>,
}

impl Drop for Replica {
    fn drop(&mut self) {
        let _ = self.killed_sender.send(self.id);
    }
}

impl Replica {
    pub fn id(&self) -> ReplicaId {
        self.id
    }

    pub fn collect_changes(&self) -> Option<ReplicaCollectChanges> {
        ReplicaCollectChanges::new(self)
    }

    pub fn apply_changes(&self) -> Option<ReplicaApplyChanges> {
        ReplicaApplyChanges::new(self)
    }

    pub fn rpc_sender(&self) -> Option<ReplicaRpcSender> {
        self.rpc_sender.as_ref().map(|sender| ReplicaRpcSender {
            sender: sender.clone(),
            recepients: Default::default(),
        })
    }

    pub fn rpc_receive(&self) -> Option<Result<RpcPartialDecoder, Box<dyn Error>>> {
        let receiver = self.rpc_receiver.as_ref()?;
        let buffer = receiver.try_recv()?;
        Some(RpcPartialDecoder::new(buffer.data))
    }
}

pub struct ReplicaCollectChanges {
    sender: Sender<ProtocolPacketData>,
    buffer: Option<Cursor<Vec<u8>>>,
    recepients: PacketRecepients,
}

impl Drop for ReplicaCollectChanges {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            let buffer = buffer.into_inner();
            if !buffer.is_empty() {
                let _ = self.sender.send(ProtocolPacketData {
                    data: buffer,
                    recepients: self.recepients.clone(),
                });
            }
        }
    }
}

impl ReplicaCollectChanges {
    pub fn new(replica: &Replica) -> Option<Self> {
        Some(Self {
            sender: replica.change_sender.as_ref().cloned()?,
            buffer: Some(Default::default()),
            recepients: Default::default(),
        })
    }

    pub fn size(&self) -> usize {
        self.buffer
            .as_ref()
            .map(|b| b.get_ref().len())
            .unwrap_or_default()
    }

    pub fn recepient(mut self, engine_id: EngineId) -> Self {
        self.recepients.push(engine_id);
        self
    }

    pub fn recepients(mut self, engine_ids: impl IntoIterator<Item = EngineId>) -> Self {
        self.recepients.extend(engine_ids);
        self
    }

    pub fn collect_replicated<P, T>(
        &mut self,
        replicated: &mut Replicated<P, T>,
    ) -> Result<(), Box<dyn Error>>
    where
        P: ReplicationPolicy<T>,
        T: Replicable,
    {
        if let Some(buffer) = &mut self.buffer {
            Replicated::collect_changes(replicated, buffer)?;
        }
        Ok(())
    }

    pub fn collect_replicable<T>(&mut self, replicable: &T) -> Result<(), Box<dyn Error>>
    where
        T: Replicable,
    {
        if let Some(buffer) = &mut self.buffer {
            replicable.collect_changes(buffer)?;
        }
        Ok(())
    }
}

pub struct ReplicaApplyChanges {
    buffer: Vec<u8>,
}

impl ReplicaApplyChanges {
    pub fn new(replica: &Replica) -> Option<Self> {
        if let Some(receiver) = &replica.change_receiver {
            receiver.try_recv().map(|buffer| Self {
                buffer: buffer.data,
            })
        } else {
            None
        }
    }

    pub fn size(&self) -> usize {
        self.buffer.len()
    }

    pub fn apply_replicated<P, T>(
        &mut self,
        replicated: &mut Replicated<P, T>,
    ) -> Result<(), Box<dyn Error>>
    where
        P: ReplicationPolicy<T>,
        T: Replicable,
    {
        let mut cursor = Cursor::new(self.buffer.as_slice());
        Replicated::apply_changes(replicated, &mut cursor)?;
        self.buffer.drain(0..cursor.position() as usize);
        Ok(())
    }

    pub fn apply_replicable<P, T>(&mut self, replicable: &mut T) -> Result<(), Box<dyn Error>>
    where
        P: ReplicationPolicy<T>,
        T: Replicable,
    {
        let mut cursor = Cursor::new(self.buffer.as_slice());
        replicable.apply_changes(&mut cursor)?;
        self.buffer.drain(0..cursor.position() as usize);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        channel::{ChannelId, ChannelMode},
        peer::{PeerBuildResult, PeerBuilder, PeerId, PeerInfo, PeerRoleId},
        replication::HashReplicated,
    };

    #[test]
    fn test_replica_set_bind_unbind() {
        let (meeting_sender, _) = unbounded();
        let peer_a = PeerBuilder::new(Peer::new(
            PeerInfo {
                peer_id: PeerId::new(0),
                role_id: PeerRoleId::new(0),
                remote: false,
            },
            meeting_sender,
        ))
        .bind_write::<ReplicationBuffer, ReplicationBuffer>(
            ChannelId::new(0),
            ChannelMode::ReliableOrdered,
            None,
        )
        .build()
        .peer;

        let (meeting_sender, _) = unbounded();
        let peer_b = PeerBuilder::new(Peer::new(
            PeerInfo {
                peer_id: PeerId::new(0),
                role_id: PeerRoleId::new(0),
                remote: false,
            },
            meeting_sender,
        ))
        .bind_read::<ReplicationBuffer, ReplicationBuffer>(
            ChannelId::new(0),
            ChannelMode::ReliableOrdered,
            None,
        )
        .build()
        .peer;

        let mut set = ReplicaSet::default();
        set.bind_peer(&peer_a, Some(ChannelId::new(0)), None);

        assert!(set.change_sender.is_some());
        assert!(set.change_receiver.is_none());

        set.bind_peer(&peer_b, Some(ChannelId::new(0)), None);
        assert!(set.change_sender.is_none());
        assert!(set.change_receiver.is_some());

        set.unbind();
        assert!(set.change_sender.is_none());
        assert!(set.change_receiver.is_none());
    }

    #[test]
    fn test_replica_changes() {
        let (meeting_sender, _meeting_receiver) = unbounded();

        let PeerBuildResult {
            peer,
            mut channels,
            descriptor,
        } = PeerBuilder::new(Peer::new(
            PeerInfo {
                peer_id: PeerId::new(0),
                role_id: PeerRoleId::new(0),
                remote: false,
            },
            meeting_sender,
        ))
        .bind_read_write::<ReplicationBuffer, ReplicationBuffer>(
            ChannelId::new(0),
            ChannelMode::ReliableOrdered,
            None,
        )
        .build();

        let mut data = HashReplicated::new(42u32);
        let mut set = ReplicaSet::default();
        set.bind_peer(&peer, Some(ChannelId::new(0)), None);
        let replica = set.create(ReplicaId::new(0)).unwrap();

        // Collect changes and send them over the channel.
        replica
            .collect_changes()
            .unwrap()
            .collect_replicated(&mut data)
            .unwrap();

        // Pump the channels.
        set.maintain();
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
        assert_eq!(packet.data.len(), 20);

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
        set.maintain();

        *data = 0;
        assert_eq!(*data, 0);

        // Apply changes received from the channel.
        replica
            .apply_changes()
            .unwrap()
            .apply_replicated(&mut data)
            .unwrap();
        assert_eq!(*data, 42);
    }

    #[test]
    fn test_replica_rpc() {
        fn greet(name: &str) {
            println!("Hello, {}!", name);
        }

        let (meeting_sender, _meeting_receiver) = unbounded();

        let PeerBuildResult {
            peer,
            mut channels,
            descriptor,
        } = PeerBuilder::new(Peer::new(
            PeerInfo {
                peer_id: PeerId::new(0),
                role_id: PeerRoleId::new(0),
                remote: false,
            },
            meeting_sender,
        ))
        .bind_read_write::<ReplicationBuffer, ReplicationBuffer>(
            ChannelId::new(0),
            ChannelMode::ReliableOrdered,
            None,
        )
        .build();

        let mut set = ReplicaSet::default();
        set.bind_peer(&peer, None, Some(ChannelId::new(0)));
        let replica = set.create(ReplicaId::new(0)).unwrap();

        // Send an RPC over the channel.
        replica
            .rpc_sender()
            .unwrap()
            .send(Rpc::<(), String>::new("greet", "Alice".to_owned()))
            .unwrap();

        // Pump the channels.
        set.maintain();
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
        assert_eq!(packet.data.len(), 61);

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
        set.maintain();

        // Receive the RPC.
        let rpc = replica
            .rpc_receive()
            .unwrap()
            .unwrap()
            .complete::<(), String>()
            .unwrap();
        let (call, input) = rpc.call().unwrap();
        assert_eq!(call.procedure(), "greet");
        assert_eq!(input.as_str(), "Alice");

        // Execute the RPC.
        greet(&input);
        call.respond(()).result().unwrap();
    }
}
