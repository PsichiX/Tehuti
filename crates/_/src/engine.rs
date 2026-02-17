use crate::{
    buffer::Buffer,
    channel::{ChannelId, ChannelMode},
    event::{Receiver, Sender},
    peer::PeerInfo,
    protocol::ProtocolPacketData,
    replication::Replicable,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, error::Error};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EngineId(u128);

impl EngineId {
    pub const fn new(id: u128) -> Self {
        Self(id)
    }

    pub const fn id(&self) -> u128 {
        self.0
    }
}

impl Replicable for EngineId {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#engine:{}", self.0)
    }
}

#[derive(Debug)]
pub struct EnginePacketSender {
    pub mode: ChannelMode,
    pub sender: Sender<ProtocolPacketData>,
}

#[derive(Debug)]
pub struct EnginePacketReceiver {
    pub mode: ChannelMode,
    pub receiver: Receiver<ProtocolPacketData>,
}

#[derive(Debug)]
pub struct EnginePeerDescriptor {
    pub info: PeerInfo,
    pub packet_senders: BTreeMap<ChannelId, EnginePacketSender>,
    pub packet_receivers: BTreeMap<ChannelId, EnginePacketReceiver>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_async() {
        fn is_send<T: Send>() {}

        is_send::<EnginePacketSender>();
        is_send::<EnginePacketReceiver>();
        is_send::<EnginePeerDescriptor>();
    }
}
