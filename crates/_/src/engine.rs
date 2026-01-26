use crate::{
    channel::{ChannelId, ChannelMode},
    meeting::{Meeting, MeetingInterface},
    peer::{PeerFactory, PeerInfo},
};
use flume::{Receiver, Sender};
use std::{collections::BTreeMap, error::Error, sync::Arc};

pub trait EngineMeeting<MeetingConfig> {
    fn request_meeting(
        &mut self,
        factory: Arc<PeerFactory>,
        config: MeetingConfig,
        reply_sender: Sender<Result<(Meeting, MeetingInterface), Box<dyn Error + Send>>>,
    ) -> Result<(), Box<dyn Error>>;
}

#[derive(Debug)]
pub struct EnginePacketSender {
    pub mode: ChannelMode,
    pub sender: Sender<Vec<u8>>,
}

#[derive(Debug)]
pub struct EnginePacketReceiver {
    pub mode: ChannelMode,
    pub receiver: Receiver<Vec<u8>>,
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
