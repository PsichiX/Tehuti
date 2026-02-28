use crate::{
    buffer::Buffer,
    channel::{ChannelId, ChannelMode},
    event::{Duplex, Receiver, Sender, unbounded},
    meeting::{Meeting, MeetingEngineEvent, MeetingInterface, MeetingInterfaceResult},
    peer::{PeerFactory, PeerId, PeerInfo},
    protocol::{ProtocolControlFrame, ProtocolFrame, ProtocolPacketData, ProtocolPacketFrame},
    replication::Replicable,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    error::Error,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EngineId(u128);

impl EngineId {
    pub fn uuid() -> Self {
        Self::new(typid::ID::<()>::default().uuid().as_u128())
    }

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

pub enum EngineMeetingEvent<Key: std::fmt::Display + Ord> {
    RegisterEngine {
        key: Key,
        engine_id: EngineId,
        frames: Duplex<ProtocolFrame>,
    },
    UnregisterEngine {
        key: Key,
    },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EngineMeetingConfig {
    pub warn_unhandled_control_frame: bool,
    pub warn_duplicate_peer_creation: bool,
    pub warn_unknown_peer_destruction: bool,
    pub warn_unknown_peer_packet: bool,
    pub warn_unknown_channel_packet: bool,
    pub warn_unhandled_engine_event: bool,
}

impl EngineMeetingConfig {
    pub fn disable_all_warnings(mut self) -> Self {
        self.warn_unhandled_control_frame = false;
        self.warn_duplicate_peer_creation = false;
        self.warn_unknown_peer_destruction = false;
        self.warn_unknown_peer_packet = false;
        self.warn_unknown_channel_packet = false;
        self.warn_unhandled_engine_event = false;
        self
    }

    pub fn enable_all_warnings(mut self) -> Self {
        self.warn_unhandled_control_frame = true;
        self.warn_duplicate_peer_creation = true;
        self.warn_unknown_peer_destruction = true;
        self.warn_unknown_peer_packet = true;
        self.warn_unknown_channel_packet = true;
        self.warn_unhandled_engine_event = true;
        self
    }

    pub fn warn_unhandled_control_frame(mut self, value: bool) -> Self {
        self.warn_unhandled_control_frame = value;
        self
    }

    pub fn warn_duplicate_peer_creation(mut self, value: bool) -> Self {
        self.warn_duplicate_peer_creation = value;
        self
    }

    pub fn warn_unknown_peer_destruction(mut self, value: bool) -> Self {
        self.warn_unknown_peer_destruction = value;
        self
    }

    pub fn warn_unknown_peer_packet(mut self, value: bool) -> Self {
        self.warn_unknown_peer_packet = value;
        self
    }

    pub fn warn_unknown_channel_packet(mut self, value: bool) -> Self {
        self.warn_unknown_channel_packet = value;
        self
    }

    pub fn warn_unhandled_engine_event(mut self, value: bool) -> Self {
        self.warn_unhandled_engine_event = value;
        self
    }
}

pub struct EngineMeetingResult<Key: std::fmt::Display + Ord> {
    pub meeting: EngineMeeting<Key>,
    pub interface: MeetingInterface,
    pub events_sender: Sender<EngineMeetingEvent<Key>>,
}

pub struct EngineMeeting<Key: std::fmt::Display + Ord> {
    pub config: EngineMeetingConfig,
    meeting: Meeting,
    engine_event: Duplex<MeetingEngineEvent>,
    peers: BTreeMap<PeerId, EnginePeerDescriptor>,
    engines: BTreeMap<Key, (Duplex<ProtocolFrame>, EngineId)>,
    events_receiver: Receiver<EngineMeetingEvent<Key>>,
}

impl<Key: std::fmt::Display + Ord> EngineMeeting<Key> {
    pub fn make(
        name: impl ToString,
        config: EngineMeetingConfig,
        factory: Arc<PeerFactory>,
    ) -> EngineMeetingResult<Key> {
        let MeetingInterfaceResult {
            meeting,
            interface,
            engine_event,
        } = MeetingInterface::make(factory, name);
        let (events_sender, events_receiver) = unbounded();
        EngineMeetingResult {
            meeting: Self {
                config,
                meeting,
                engine_event,
                peers: Default::default(),
                engines: Default::default(),
                events_receiver,
            },
            interface,
            events_sender,
        }
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        // Handle engine meeting control events.
        for event in self.events_receiver.iter() {
            match event {
                EngineMeetingEvent::RegisterEngine {
                    key,
                    engine_id,
                    frames,
                } => {
                    tracing::event!(
                        target: "tehuti::engine::meeting",
                        tracing::Level::TRACE,
                        "Meeting registering engine: {}, engine ID: {:?}",
                        key,
                        engine_id,
                    );
                    for descriptor in self.peers.values() {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::TRACE,
                            "Meeting engine: {} register sending create peer {} with role {}",
                            key,
                            descriptor.info.peer_id,
                            descriptor.info.role_id,
                        );
                        let frame = ProtocolFrame::Control(ProtocolControlFrame::CreatePeer(
                            descriptor.info.peer_id,
                            descriptor.info.role_id,
                        ));
                        frames.sender.send(frame).map_err(|err| {
                            format!("Meeting engine {} frame sender error: {}", key, err)
                        })?;
                    }
                    self.engines.insert(key, (frames, engine_id));
                }
                EngineMeetingEvent::UnregisterEngine { key } => {
                    tracing::event!(
                        target: "tehuti::engine::meeting",
                        tracing::Level::TRACE,
                        "Meeting unregistering engine: {}",
                        key,
                    );
                    self.engines.remove(&key);
                }
            }
        }

        // Process receiving frames from all engines.
        for (key, (frames, _)) in &self.engines {
            for frame in frames.receiver.iter() {
                match frame {
                    ProtocolFrame::Control(frame) => {
                        for (key2, (frames2, _)) in &self.engines {
                            if key != key2 {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} forwarding control frame to {}: {:?}",
                                    key,
                                    key2,
                                    frame,
                                );
                                frames2
                                    .sender
                                    .send(ProtocolFrame::Control(frame.clone()))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting engine {} frame sender error: {}",
                                            key, err
                                        )
                                    })?;
                            }
                        }

                        match frame {
                            ProtocolControlFrame::CreatePeer(peer_id, peer_role_id) => {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} got create peer {:?} with role {:?}",
                                    key,
                                    peer_id,
                                    peer_role_id,
                                );
                                self.engine_event
                                    .sender
                                    .send(MeetingEngineEvent::PeerJoined(peer_id, peer_role_id))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting engine {} outside engine sender error: {err}",
                                            key
                                        )
                                    })
                                    .unwrap();
                            }
                            ProtocolControlFrame::DestroyPeer(peer_id) => {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} got destroy peer {:?}",
                                    key,
                                    peer_id,
                                );
                                self.engine_event
                                    .sender
                                    .send(MeetingEngineEvent::PeerLeft(peer_id))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting engine {} outside engine sender error: {err}",
                                            key
                                        )
                                    })
                                    .unwrap();
                            }
                            _ => {
                                if self.config.warn_unhandled_control_frame {
                                    tracing::event!(
                                        target: "tehuti::engine::meeting",
                                        tracing::Level::WARN,
                                        "Meeting engine {} got unhandled control frame: {:?}",
                                        key,
                                        frame,
                                    );
                                }
                            }
                        }
                    }
                    ProtocolFrame::Packet(frame) => {
                        for (key2, (frames2, engine_id2)) in &self.engines {
                            if !frame.data.recepients.is_empty()
                                && !frame.data.recepients.contains(engine_id2)
                            {
                                continue;
                            }
                            if key != key2 {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} forwarding packet frame for peer {:?} channel {:?} to {}. {} bytes",
                                    key,
                                    frame.peer_id,
                                    frame.channel_id,
                                    key2,
                                    frame.data.data.len(),
                                );
                                frames2
                                    .sender
                                    .send(ProtocolFrame::Packet(frame.clone()))
                                    .map_err(|err| {
                                        format!("Meeting engine {} frame sender error: {err}", key2)
                                    })?;
                            }
                        }

                        if let Some(peer) = self.peers.get(&frame.peer_id) {
                            if let Some(sender) = peer.packet_senders.get(&frame.channel_id) {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} got packet frame for peer {:?} channel {:?}: {} bytes",
                                    key,
                                    frame.peer_id,
                                    frame.channel_id,
                                    frame.data.data.len(),
                                );
                                sender
                                    .sender
                                    .send(frame.data)
                                    .map_err(|err| {
                                        format!("Meeting engine {} packet sender error: {err}", key)
                                    })
                                    .unwrap();
                            } else if self.config.warn_unknown_channel_packet {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::WARN,
                                    "Meeting engine {} got packet frame for unknown channel {:?} of peer {:?}",
                                    key,
                                    frame.channel_id,
                                    frame.peer_id,
                                );
                            }
                        } else if self.config.warn_unknown_peer_packet {
                            tracing::event!(
                                target: "tehuti::engine::meeting",
                                tracing::Level::WARN,
                                "Meeting engine {} got packet frame for unknown peer {:?}",
                                key,
                                frame.peer_id,
                            );
                        }
                    }
                }
            }
        }

        // Handle sending packets from peers to all sessions.
        for peer in self.peers.values() {
            for (channel_id, receiver) in &peer.packet_receivers {
                for data in receiver.receiver.iter() {
                    let size = data.data.len();
                    let recepients = data.recepients.to_owned();
                    let frame = ProtocolFrame::Packet(ProtocolPacketFrame {
                        peer_id: peer.info.peer_id,
                        channel_id: *channel_id,
                        data,
                    });
                    for (key, (frames, engine_id)) in &self.engines {
                        if !recepients.is_empty() && !recepients.contains(engine_id) {
                            continue;
                        }
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::TRACE,
                            "Meeting engine {} sending packet frame for peer {:?} channel {:?}. {} bytes",
                            key,
                            peer.info.peer_id,
                            channel_id,
                            size,
                        );
                        frames.sender.send(frame.clone()).map_err(|err| {
                            format!("Meeting engine {} frame sender error: {}", key, err)
                        })?;
                    }
                }
            }
        }

        // Handle meeting pump.
        if let Err(err) = self.meeting.pump_all() {
            tracing::event!(
                target: "tehuti::engine::meeting",
                tracing::Level::ERROR,
                "Meeting encountered error: {}. Terminating",
                err,
            );
            return Err(err);
        }

        // Handle meeting engine events.
        for event in self.engine_event.receiver.iter() {
            match event {
                MeetingEngineEvent::MeetingDestroyed => {
                    tracing::event!(
                        target: "tehuti::engine::meeting",
                        tracing::Level::TRACE,
                        "Meeting got destroyed. Terminating",
                    );
                    return Err("Meeting destroyed".into());
                }
                MeetingEngineEvent::PeerCreated(descriptor) => {
                    if let Entry::Vacant(entry) = self.peers.entry(descriptor.info.peer_id) {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::TRACE,
                            "Meeting created peer {:?}",
                            descriptor.info.peer_id,
                        );
                        if !descriptor.info.remote {
                            let frame = ProtocolFrame::Control(ProtocolControlFrame::CreatePeer(
                                descriptor.info.peer_id,
                                descriptor.info.role_id,
                            ));
                            for (key, (frames, _)) in &self.engines {
                                tracing::event!(
                                    target: "tehuti::engine::meeting",
                                    tracing::Level::TRACE,
                                    "Meeting engine {} sending create peer {:?} with role {:?}",
                                    key,
                                    descriptor.info.peer_id,
                                    descriptor.info.role_id,
                                );
                                frames.sender.send(frame.clone()).map_err(|err| {
                                    format!("Meeting engine {} frame sender error: {}", key, err)
                                })?;
                            }
                        }
                        entry.insert(descriptor);
                    } else if self.config.warn_duplicate_peer_creation {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::WARN,
                            "Meeting tries to create duplicate peer {:?} created",
                            descriptor.info.peer_id,
                        );
                    }
                }
                MeetingEngineEvent::PeerDestroyed(peer_id) => {
                    if self.peers.contains_key(&peer_id) {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::TRACE,
                            "Meeting destroyed peer {:?}",
                            peer_id,
                        );
                        let frame =
                            ProtocolFrame::Control(ProtocolControlFrame::DestroyPeer(peer_id));
                        for (key, (frames, _)) in &self.engines {
                            tracing::event!(
                                target: "tehuti::engine::meeting",
                                tracing::Level::TRACE,
                                "Meeting engine {} sending destroy peer {:?}",
                                key,
                                peer_id,
                            );
                            frames.sender.send(frame.clone()).map_err(|err| {
                                format!("Meeting engine {} frame sender error: {}", key, err)
                            })?;
                        }
                        self.peers.remove(&peer_id);
                    } else if self.config.warn_unknown_peer_destruction {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::WARN,
                            "Meeting tries to destroy unknown peer {:?} destroyed",
                            peer_id,
                        );
                    }
                }
                event => {
                    if self.config.warn_unhandled_engine_event {
                        tracing::event!(
                            target: "tehuti::engine::meeting",
                            tracing::Level::WARN,
                            "Meeting got unhandled engine event: {:?}",
                            event,
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

impl<Key: std::fmt::Display + Ord> Future for EngineMeeting<Key> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.maintain() {
            Ok(_) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => {
                tracing::event!(
                    target: "tehuti::engine::meeting",
                    tracing::Level::ERROR,
                    "Meeting encountered error: {}. Terminating",
                    e,
                );
                Poll::Ready(())
            }
        }
    }
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
