use std::{
    collections::{BTreeMap, btree_map::Entry},
    error::Error,
    ops::Range,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tehuti::{
    Duplex,
    engine::EnginePeerDescriptor,
    meeting::{Meeting, MeetingEngineEvent, MeetingUserEvent},
    peer::{PeerFactory, PeerId},
    protocol::{ProtocolControlFrame, ProtocolPacketFrame},
    third_party::flume::{Sender, unbounded},
};
use tracing::level_filters::LevelFilter;

#[derive(Debug, Clone)]
enum MockNetworkMessage {
    ControlFrame {
        delay: Duration,
        frame: ProtocolControlFrame,
    },
    PacketFrame {
        delay: Duration,
        frame: ProtocolPacketFrame,
    },
}

struct MockNetworkPortNode {
    engine_control: Duplex<ProtocolControlFrame>,
    engine_packet: Duplex<ProtocolPacketFrame>,
    mailbox: Vec<MockNetworkMessage>,
    config: MockNetworkPortConfig,
}

#[derive(Default)]
pub struct MockNetworkPortConfig {
    pub packet_lost_chance: f32,
    pub base_latency: Duration,
    pub latency: Range<Duration>,
}

impl MockNetworkPortConfig {
    pub fn should_lose_packet(&self) -> bool {
        rand::random_bool(self.packet_lost_chance as _)
    }

    pub fn latency(&self) -> Duration {
        if self.latency.is_empty() {
            self.base_latency
        } else {
            self.base_latency + rand::random_range(self.latency.clone())
        }
    }
}

enum MockNetworkRequest {
    Terminate,
    CreatePort {
        id: String,
        config: MockNetworkPortConfig,
        reply: Sender<MockNetworkPort>,
    },
    DestroyPort {
        id: String,
    },
    ReconfigurePort {
        id: String,
        config: MockNetworkPortConfig,
    },
}

/// All ports in the mock network broadcast messages to each other.
/// There is no directed connections between only pair of ports.
pub struct MockNetwork {
    requests: Sender<MockNetworkRequest>,
    thread: Option<JoinHandle<()>>,
}

impl Default for MockNetwork {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

impl MockNetwork {
    pub fn new(interval: Duration) -> Self {
        let (sender, receiver) = unbounded();
        let thread = std::thread::spawn(move || {
            tracing::event!(
                target: "tehuti::mock::network",
                tracing::Level::TRACE,
                "Mock network started in thread {:?}",
                std::thread::current().id()
            );

            let mut timer = Instant::now();
            let mut nodes = BTreeMap::new();

            'main: loop {
                let delta_time = timer.elapsed();
                timer = Instant::now();

                // Handle actions from the outside.
                for action in receiver.drain() {
                    match action {
                        MockNetworkRequest::Terminate => {
                            tracing::event!(
                                target: "tehuti::mock::network",
                                tracing::Level::TRACE,
                                "Mock network terminating in thread {:?}",
                                std::thread::current().id()
                            );
                            break 'main;
                        }
                        MockNetworkRequest::CreatePort { id, config, reply } => {
                            let (engine_control, network_control) = Duplex::crossing_unbounded();
                            let (engine_packet, network_packet) = Duplex::crossing_unbounded();
                            let result = MockNetworkPort {
                                network_control,
                                network_packet,
                            };
                            nodes.insert(
                                id.to_owned(),
                                MockNetworkPortNode {
                                    engine_control,
                                    engine_packet,
                                    mailbox: Default::default(),
                                    config,
                                },
                            );
                            tracing::event!(
                                target: "tehuti::mock::network",
                                tracing::Level::TRACE,
                                "Mock network port '{}' created",
                                id
                            );
                            reply
                                .send(result)
                                .map_err(|err| format!("Reply sender error: {err}"))
                                .unwrap();
                        }
                        MockNetworkRequest::DestroyPort { id } => {
                            nodes.remove(&id);
                            tracing::event!(
                                target: "tehuti::mock::network",
                                tracing::Level::TRACE,
                                "Mock network port '{}' destroyed",
                                id
                            );
                        }
                        MockNetworkRequest::ReconfigurePort { id, config } => {
                            if let Some(node) = nodes.get_mut(&id) {
                                node.config = config;
                                tracing::event!(
                                    target: "tehuti::mock::network",
                                    tracing::Level::TRACE,
                                    "Mock network port '{}' reconfigured",
                                    id
                                );
                            }
                        }
                    }
                }

                // Collecting messages from engines.
                let messages = nodes
                    .iter()
                    .flat_map(|(id, node)| {
                        node.engine_control
                            .receiver
                            .try_iter()
                            .filter(|_| !node.config.should_lose_packet())
                            .map(|frame| {
                                tracing::event!(
                                    target: "tehuti::mock::network",
                                    tracing::Level::TRACE,
                                    "Mock network received control frame {:?} from node '{}'",
                                    frame,
                                    id.to_owned()
                                );
                                (id.to_owned(), MockNetworkMessage::ControlFrame {
                                    delay: node.config.latency(),
                                    frame,
                                })
                            })
                            .chain(
                                node.engine_packet
                                    .receiver
                                    .try_iter()
                                    .filter(|_| !node.config.should_lose_packet())
                                    .map(|frame| {
                                        tracing::event!(
                                            target: "tehuti::mock::network",
                                            tracing::Level::TRACE,
                                            "Mock network received packet frame {:?} from node '{}'",
                                            frame,
                                            id.to_owned()
                                        );
                                        (id.to_owned(), MockNetworkMessage::PacketFrame {
                                            delay: node.config.latency(),
                                            frame,
                                        })
                                    }),
                            )
                    })
                    .collect::<Vec<_>>();

                // Distributing collected messages to nodes mailboxes.
                for (source, message) in messages {
                    for (target, node) in &mut nodes {
                        if &source != target {
                            node.mailbox.push(message.clone());
                        }
                    }
                }

                // Forwarding messages from node mailbox to engine.
                for (id, node) in &mut nodes {
                    let mut remaining_mailbox = Vec::new();
                    for message in node.mailbox.drain(..) {
                        match message {
                            MockNetworkMessage::ControlFrame { mut delay, frame } => {
                                delay = delay.saturating_sub(delta_time);
                                if delay <= Duration::ZERO {
                                    tracing::event!(
                                        target: "tehuti::mock::network",
                                        tracing::Level::TRACE,
                                        "Mock network send control frame {:?} to node '{}'",
                                        frame,
                                        id
                                    );
                                    if let Err(err) = node.engine_control.sender.send(frame) {
                                        tracing::event!(
                                            target: "tehuti::mock::network",
                                            tracing::Level::ERROR,
                                            "Mock network failed to send control frame to node '{}': {}",
                                            id,
                                            err
                                        );
                                    }
                                } else {
                                    remaining_mailbox
                                        .push(MockNetworkMessage::ControlFrame { delay, frame });
                                }
                            }
                            MockNetworkMessage::PacketFrame { mut delay, frame } => {
                                delay = delay.saturating_sub(delta_time);
                                if delay <= Duration::ZERO {
                                    tracing::event!(
                                        target: "tehuti::mock::network",
                                        tracing::Level::TRACE,
                                        "Mock network send packet frame {:?} to node '{}'",
                                        frame,
                                        id
                                    );
                                    if let Err(err) = node.engine_packet.sender.send(frame) {
                                        tracing::event!(
                                            target: "tehuti::mock::network",
                                            tracing::Level::ERROR,
                                            "Mock network failed to send packet frame to node '{}': {}",
                                            id,
                                            err
                                        );
                                    }
                                } else {
                                    remaining_mailbox
                                        .push(MockNetworkMessage::PacketFrame { delay, frame });
                                }
                            }
                        }
                    }
                    node.mailbox.extend(remaining_mailbox);
                }

                std::thread::sleep(interval);
            }

            tracing::event!(
                target: "tehuti::mock::network",
                tracing::Level::TRACE,
                "Mock network stopped in thread {:?}",
                std::thread::current().id()
            );
        });
        MockNetwork {
            requests: sender,
            thread: Some(thread),
        }
    }

    pub fn open_port(
        &self,
        id: impl ToString,
        config: MockNetworkPortConfig,
    ) -> Result<MockNetworkPort, Box<dyn Error>> {
        let (reply_sender, reply_receiver) = unbounded();
        self.requests
            .send(MockNetworkRequest::CreatePort {
                id: id.to_string(),
                config,
                reply: reply_sender,
            })
            .map_err(|err| format!("Network request sender error: {err}"))?;
        Ok(reply_receiver.recv()?)
    }

    pub fn close_port(&self, id: &str) -> Result<(), Box<dyn Error>> {
        self.requests
            .send(MockNetworkRequest::DestroyPort { id: id.to_string() })
            .map_err(|err| format!("Network request sender error: {err}"))?;
        Ok(())
    }

    pub fn reconfigure_port(
        &self,
        id: &str,
        config: MockNetworkPortConfig,
    ) -> Result<(), Box<dyn Error>> {
        self.requests
            .send(MockNetworkRequest::ReconfigurePort {
                id: id.to_string(),
                config,
            })
            .map_err(|err| format!("Network request sender error: {err}"))?;
        Ok(())
    }

    pub fn join(mut self) {
        let _ = self.requests.send(MockNetworkRequest::Terminate);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct MockNetworkPort {
    pub network_control: Duplex<ProtocolControlFrame>,
    pub network_packet: Duplex<ProtocolPacketFrame>,
}

pub struct MockMeetingCommunication {
    id: usize,
    pub user_event: Duplex<MeetingUserEvent>,
}

impl MockMeetingCommunication {
    pub fn id(&self) -> usize {
        self.id
    }
}

struct MockMeeting {
    terminate: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for MockMeeting {
    fn drop(&mut self) {
        let _ = self.terminate.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl MockMeeting {
    pub fn new(
        id: usize,
        factory: Arc<PeerFactory>,
        port: MockNetworkPort,
        interval: Duration,
    ) -> (Self, Duplex<MeetingUserEvent>) {
        let (inside_engine, outside_engine) = Duplex::crossing_unbounded();
        let (inside_user, outside_user) = Duplex::crossing_unbounded();
        let (termination_sender, termination_receiver) = unbounded();
        let thread = std::thread::spawn(move || {
            tracing::event!(
                target: "tehuti::mock::meeting",
                tracing::Level::TRACE,
                "Mock meeting {} started in thread {:?}",
                id,
                std::thread::current().id()
            );

            let mut meeting = Meeting::new(
                factory,
                inside_engine,
                inside_user,
                format!("{}-{:?}", id, std::thread::current().id()),
            );
            let mut peers = BTreeMap::<PeerId, EnginePeerDescriptor>::new();

            loop {
                if termination_receiver.drain().next().is_some() {
                    tracing::event!(
                        target: "tehuti::mock::meeting",
                        tracing::Level::TRACE,
                        "Mock meeting {} terminating in thread {:?}",
                        id,
                        std::thread::current().id()
                    );
                    break;
                }

                for frame in port.network_control.receiver.drain() {
                    match frame {
                        ProtocolControlFrame::CreatePeer(peer_id, peer_role_id) => {
                            outside_engine
                                .sender
                                .send(MeetingEngineEvent::PeerJoined(peer_id, peer_role_id))
                                .map_err(|err| {
                                    format!("Machine outside engine sender error: {err}")
                                })
                                .unwrap();
                        }
                        ProtocolControlFrame::DestroyPeer(peer_id) => {
                            outside_engine
                                .sender
                                .send(MeetingEngineEvent::PeerLeft(peer_id))
                                .map_err(|err| {
                                    format!("Machine outside engine sender error: {err}")
                                })
                                .unwrap();
                        }
                        _ => {
                            tracing::event!(
                                target: "tehuti::mock::meeting",
                                tracing::Level::WARN,
                                "Mock meeting {} got unhandled control frame: {:?} in thread {:?}",
                                id,
                                frame,
                                std::thread::current().id()
                            );
                        }
                    }
                }

                for frame in port.network_packet.receiver.drain() {
                    if let Some(peer) = peers.get(&frame.peer_id) {
                        if let Some(sender) = peer.packet_senders.get(&frame.channel_id) {
                            sender
                                .sender
                                .send(frame.data)
                                .map_err(|err| format!("Machine packet sender error: {err}"))
                                .unwrap();
                        } else {
                            tracing::event!(
                                target: "tehuti::mock::meeting",
                                tracing::Level::WARN,
                                "Mock meeting {} got packet frame for unknown channel {:?} of peer {:?} in thread {:?}",
                                id,
                                frame.channel_id,
                                frame.peer_id,
                                std::thread::current().id()
                            );
                        }
                    } else {
                        tracing::event!(
                            target: "tehuti::mock::meeting",
                            tracing::Level::WARN,
                            "Mock meeting {} got packet frame for unknown peer {:?} in thread {:?}",
                            id,
                            frame.peer_id,
                            std::thread::current().id()
                        );
                    }
                }

                for peer in peers.values() {
                    for (channel_id, receiver) in &peer.packet_receivers {
                        for data in receiver.receiver.drain() {
                            port.network_packet
                                .sender
                                .send(ProtocolPacketFrame {
                                    peer_id: peer.info.peer_id,
                                    channel_id: *channel_id,
                                    data,
                                })
                                .map_err(|err| format!("Network port packet sender error: {err}"))
                                .unwrap();
                        }
                    }
                }

                if let Err(err) = meeting.pump_all() {
                    tracing::event!(
                        target: "tehuti::mock::meeting",
                        tracing::Level::ERROR,
                        "Mock meeting {} encountered error: {} in thread {:?}. Terminating",
                        id,
                        err,
                        std::thread::current().id()
                    );
                    break;
                }

                for event in outside_engine.receiver.drain() {
                    match event {
                        MeetingEngineEvent::MeetingDestroyed => {
                            tracing::event!(
                                target: "tehuti::mock::meeting",
                                tracing::Level::TRACE,
                                "Mock meeting {} terminating in thread {:?}",
                                id,
                                std::thread::current().id()
                            );
                            break;
                        }
                        MeetingEngineEvent::PeerCreated(descriptor) => {
                            if let Entry::Vacant(entry) = peers.entry(descriptor.info.peer_id) {
                                if !descriptor.info.remote {
                                    port.network_control
                                        .sender
                                        .send(ProtocolControlFrame::CreatePeer(
                                            descriptor.info.peer_id,
                                            descriptor.info.role_id,
                                        ))
                                        .map_err(|err| {
                                            format!("Network control sender error: {err}")
                                        })
                                        .unwrap();
                                }
                                tracing::event!(
                                    target: "tehuti::mock::meeting",
                                    tracing::Level::TRACE,
                                    "Mock meeting {} created peer {:?} in thread {:?}",
                                    id,
                                    descriptor.info.peer_id,
                                    std::thread::current().id()
                                );
                                entry.insert(descriptor);
                            } else {
                                tracing::event!(
                                    target: "tehuti::mock::meeting",
                                    tracing::Level::WARN,
                                    "Mock meeting {} got duplicate peer {:?} created in thread {:?}",
                                    id,
                                    descriptor.info.peer_id,
                                    std::thread::current().id()
                                );
                            }
                        }
                        MeetingEngineEvent::PeerDestroyed(peer_id) => {
                            if peers.contains_key(&peer_id) {
                                port.network_control
                                    .sender
                                    .send(ProtocolControlFrame::DestroyPeer(peer_id))
                                    .map_err(|err| format!("Network control sender error: {err}"))
                                    .unwrap();
                                tracing::event!(
                                    target: "tehuti::mock::meeting",
                                    tracing::Level::TRACE,
                                    "Mock meeting {} destroyed peer {:?} in thread {:?}",
                                    id,
                                    peer_id,
                                    std::thread::current().id()
                                );
                                peers.remove(&peer_id);
                            } else {
                                tracing::event!(
                                    target: "tehuti::mock::meeting",
                                    tracing::Level::WARN,
                                    "Mock meeting {} got unknown peer {:?} destroyed in thread {:?}",
                                    id,
                                    peer_id,
                                    std::thread::current().id()
                                );
                            }
                        }
                        event => {
                            tracing::event!(
                                target: "tehuti::mock::meeting",
                                tracing::Level::WARN,
                                "Mock meeting {} got unhandled engine event: {:?} in thread {:?}",
                                id,
                                event,
                                std::thread::current().id()
                            );
                        }
                    }
                }

                std::thread::sleep(interval);
            }

            tracing::event!(
                target: "tehuti::mock::meeting",
                tracing::Level::TRACE,
                "Mock meeting {} stopped in thread {:?}",
                id,
                std::thread::current().id()
            );
        });
        (
            Self {
                terminate: termination_sender,
                thread: Some(thread),
            },
            outside_user,
        )
    }
}

enum MockMachineRequest {
    Terminate,
    StartMeeting {
        factory: Arc<PeerFactory>,
        port: MockNetworkPort,
        reply: Sender<MockMeetingCommunication>,
    },
    StopMeeting {
        id: usize,
    },
}

pub struct MockMachine {
    request: Sender<MockMachineRequest>,
    thread: Option<JoinHandle<()>>,
}

impl MockMachine {
    pub fn join(mut self) {
        let _ = self.request.send(MockMachineRequest::Terminate);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Default for MockMachine {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

impl MockMachine {
    pub fn new(interval: Duration) -> Self {
        let (sender, receiver) = unbounded();
        let thread = std::thread::spawn(move || {
            tracing::event!(
                target: "tehuti::mock::machine",
                tracing::Level::TRACE,
                "Mock machine started in thread {:?}",
                std::thread::current().id()
            );

            let mut id_generator = 0;
            let mut meetings = BTreeMap::new();

            'main: loop {
                for action in receiver.drain() {
                    match action {
                        MockMachineRequest::Terminate => {
                            tracing::event!(
                                target: "tehuti::mock::machine",
                                tracing::Level::TRACE,
                                "Mock machine terminating in thread {:?}",
                                std::thread::current().id()
                            );
                            break 'main;
                        }
                        MockMachineRequest::StartMeeting {
                            factory,
                            port,
                            reply,
                        } => {
                            id_generator += 1;
                            let (meeting, outside_user) =
                                MockMeeting::new(id_generator, factory, port, interval);
                            meetings.insert(id_generator, meeting);
                            reply
                                .send(MockMeetingCommunication {
                                    id: id_generator,
                                    user_event: outside_user,
                                })
                                .map_err(|err| format!("Machine reply sender error: {err}"))
                                .unwrap();
                        }
                        MockMachineRequest::StopMeeting { id } => {
                            meetings.remove(&id);
                        }
                    }
                }

                std::thread::sleep(interval);
            }

            tracing::event!(
                target: "tehuti::mock::machine",
                tracing::Level::TRACE,
                "Mock machine stopped in thread {:?}",
                std::thread::current().id()
            );
        });
        MockMachine {
            request: sender,
            thread: Some(thread),
        }
    }

    pub fn start_meeting(
        &self,
        factory: Arc<PeerFactory>,
        port: MockNetworkPort,
    ) -> Result<MockMeetingCommunication, Box<dyn Error>> {
        let (reply_sender, reply_receiver) = unbounded();
        self.request
            .send(MockMachineRequest::StartMeeting {
                factory,
                port,
                reply: reply_sender,
            })
            .map_err(|err| format!("Machine request sender error: {err}"))?;
        Ok(reply_receiver.recv()?)
    }

    pub fn stop_meeting(&self, id: usize) -> Result<(), Box<dyn Error>> {
        self.request
            .send(MockMachineRequest::StopMeeting { id })
            .map_err(|err| format!("Machine request sender error: {err}"))?;
        Ok(())
    }
}

pub fn mock_env_tracing() {
    use tracing_subscriber::{
        Layer, fmt::layer, layer::SubscriberExt, registry, util::SubscriberInitExt,
    };

    registry()
        .with(
            layer()
                .with_writer(std::io::stdout)
                .with_filter(LevelFilter::TRACE),
        )
        .init();
}

#[macro_export]
macro_rules! mock_recv_matching {
    ($receiver:expr, $timeout:expr, $pattern:pat $(if $guard:expr)? => $extract:tt ) => {{
        let mut duration: std::time::Duration = $timeout;
        let start = std::time::Instant::now();
        let result = loop {
            let message = $receiver.recv_timeout(duration).unwrap();
            if let $pattern $(if $guard)? = message {
                break $extract;
            }
            duration = $timeout.saturating_sub(start.elapsed());
            if duration == std::time::Duration::ZERO {
                panic!("Timeout waiting for matching message");
            }
        };
        result
    }};
    ($receiver:expr, $timeout:expr, $pattern:pat $(if $guard:expr)? ) => {{
        let mut duration: std::time::Duration = $timeout;
        let start = std::time::Instant::now();
        let result = loop {
            let message = $receiver.recv_timeout(duration).unwrap();
            if let $pattern $(if $guard)? = message {
                break message;
            }
            duration = $timeout.saturating_sub(start.elapsed());
            if duration == std::time::Duration::ZERO {
                panic!("Timeout waiting for matching message");
            }
        };
        result
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{error::Error, sync::Arc};
    use tehuti::{
        channel::{ChannelId, ChannelMode},
        packet::PacketSerializer,
        peer::{PeerFactory, PeerId, PeerRoleId},
    };

    struct StringPacketSerializer;

    impl PacketSerializer<String> for StringPacketSerializer {
        fn encode(&mut self, message: &String, buffer: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
            buffer.extend_from_slice(message.as_bytes());
            Ok(())
        }

        fn decode(&mut self, buffer: &[u8]) -> Result<String, Box<dyn Error>> {
            Ok(String::from_utf8(buffer.to_vec())?)
        }
    }

    #[test]
    fn test_mock() {
        mock_env_tracing();

        // Peers role id is used to tell what channels they have rights to read
        // and/or write. This mechanism replaces fixed roles commonly used in
        // networking libraries, by allowing users to define their own roles.
        // For example Authority role might read and write on all channels,
        // while Client role might only read most and write on few channels.
        let factory = Arc::new(PeerFactory::default().with(PeerRoleId::new(0), |builder| {
            builder
                .bind_read::<String>(
                    ChannelId::new(0),
                    ChannelMode::ReliableOrdered,
                    StringPacketSerializer,
                )
                .bind_write::<String>(
                    ChannelId::new(0),
                    ChannelMode::ReliableOrdered,
                    StringPacketSerializer,
                )
        }));

        // Create network that will transmit messages between machines.
        let network = MockNetwork::default();

        // Create two machines connected via the mock network.
        // Meetings created on these machines will be able to communicate with
        // all other machines in same network.
        let port = network.open_port("a", Default::default()).unwrap();
        let machine_a = MockMachine::default();
        let meeting_a = machine_a.start_meeting(factory.clone(), port).unwrap();

        let port = network.open_port("b", Default::default()).unwrap();
        let machine_b = MockMachine::default();
        let meeting_b = machine_b.start_meeting(factory.clone(), port).unwrap();

        // Create peers in connected meeting, replicated on all machines.
        meeting_a
            .user_event
            .sender
            .send(MeetingUserEvent::PeerCreate(
                PeerId::new(0),
                PeerRoleId::new(0),
            ))
            .unwrap();

        // Gather created peers on both machines.
        let peer_a = mock_recv_matching!(
            meeting_a.user_event.receiver,
            Duration::from_secs(1),
            MeetingUserEvent::PeerAdded(peer) => peer
        );
        let peer_b = mock_recv_matching!(
            meeting_b.user_event.receiver,
            Duration::from_secs(1),
            MeetingUserEvent::PeerAdded(peer) => peer
        );

        // Send message from peer on machine A to peer on machine B.
        peer_a
            .send(ChannelId::new(0), "Hello from machine A".to_owned())
            .unwrap();

        // Receive message on peer on machine B.
        let msg = peer_b
            .recv_timeout::<String>(ChannelId::new(0), Duration::from_secs(1))
            .unwrap();
        assert_eq!(&msg, "Hello from machine A");

        // Don't let important stuff drop too early.
        drop(meeting_a);
        drop(meeting_b);
        machine_a.join();
        machine_b.join();
        network.join();
    }
}
