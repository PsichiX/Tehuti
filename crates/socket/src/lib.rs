use std::{
    collections::{BTreeMap, HashMap, btree_map::Entry},
    error::Error,
    future::pending,
    io::{Cursor, ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::Arc,
    thread::{Builder, JoinHandle, sleep},
    time::Duration,
    vec,
};
use tehuti::{
    engine::{EngineId, EnginePeerDescriptor},
    event::{Duplex, Receiver, Sender, unbounded},
    meeting::{Meeting, MeetingEngineEvent, MeetingInterface, MeetingInterfaceResult},
    peer::{PeerFactory, PeerId},
    protocol::{ProtocolControlFrame, ProtocolFrame, ProtocolPacketFrame},
};

pub enum TcpMeetingEvent {
    RegisterSession {
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        engine_id: EngineId,
        frames: Duplex<ProtocolFrame>,
    },
    UnregisterSession {
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpMeetingConfig {
    pub warn_unhandled_control_frame: bool,
    pub warn_duplicate_peer_creation: bool,
    pub warn_unknown_peer_destruction: bool,
    pub warn_unknown_peer_packet: bool,
    pub warn_unknown_channel_packet: bool,
    pub warn_unhandled_engine_event: bool,
}

impl TcpMeetingConfig {
    pub fn disable_all() -> Self {
        Self {
            warn_unhandled_control_frame: false,
            warn_duplicate_peer_creation: false,
            warn_unknown_peer_destruction: false,
            warn_unknown_peer_packet: false,
            warn_unknown_channel_packet: false,
            warn_unhandled_engine_event: false,
        }
    }

    pub fn enable_all() -> Self {
        Self {
            warn_unhandled_control_frame: true,
            warn_duplicate_peer_creation: true,
            warn_unknown_peer_destruction: true,
            warn_unknown_peer_packet: true,
            warn_unknown_channel_packet: true,
            warn_unhandled_engine_event: true,
        }
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

pub struct TcpMeetingResult {
    pub meeting: TcpMeeting,
    pub interface: MeetingInterface,
    pub events_sender: Sender<TcpMeetingEvent>,
}

pub struct TcpMeeting {
    pub config: TcpMeetingConfig,
    meeting: Meeting,
    engine_event: Duplex<MeetingEngineEvent>,
    peers: BTreeMap<PeerId, EnginePeerDescriptor>,
    sessions: HashMap<(SocketAddr, SocketAddr), (Duplex<ProtocolFrame>, EngineId)>,
    events_receiver: Receiver<TcpMeetingEvent>,
}

impl TcpMeeting {
    pub fn make(
        name: impl ToString,
        config: TcpMeetingConfig,
        factory: Arc<PeerFactory>,
    ) -> TcpMeetingResult {
        let MeetingInterfaceResult {
            meeting,
            interface,
            engine_event,
        } = MeetingInterface::make(factory, name);
        let (events_sender, events_receiver) = unbounded();
        TcpMeetingResult {
            meeting: Self {
                config,
                meeting,
                engine_event,
                peers: Default::default(),
                sessions: Default::default(),
                events_receiver,
            },
            interface,
            events_sender,
        }
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        // Handle tcp meeting control events.
        for event in self.events_receiver.iter() {
            match event {
                TcpMeetingEvent::RegisterSession {
                    local_addr,
                    peer_addr,
                    engine_id,
                    frames,
                } => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::TRACE,
                        "Meeting registering session: {:?}<->{:?}, engine ID: {:?}",
                        local_addr,
                        peer_addr,
                        engine_id,
                    );
                    for descriptor in self.peers.values() {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Meeting session: {:?}<->{:?} register sending create peer {:?} with role {:?}",
                            local_addr,
                            peer_addr,
                            descriptor.info.peer_id,
                            descriptor.info.role_id,
                        );
                        let frame = ProtocolFrame::Control(ProtocolControlFrame::CreatePeer(
                            descriptor.info.peer_id,
                            descriptor.info.role_id,
                        ));
                        frames.sender.send(frame).map_err(|err| {
                            format!(
                                "Meeting session {:?}<->{:?} frame sender error: {}",
                                local_addr, peer_addr, err
                            )
                        })?;
                    }
                    self.sessions
                        .insert((local_addr, peer_addr), (frames, engine_id));
                }
                TcpMeetingEvent::UnregisterSession {
                    local_addr,
                    peer_addr,
                } => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::TRACE,
                        "Meeting unregistering session: {:?}<->{:?}",
                        local_addr,
                        peer_addr,
                    );
                    self.sessions.remove(&(local_addr, peer_addr));
                }
            }
        }

        // Process receiving frames from all sessions.
        for ((local_addr, peer_addr), (frames, _)) in &self.sessions {
            for frame in frames.receiver.iter() {
                match frame {
                    ProtocolFrame::Control(frame) => {
                        for ((local_addr2, peer_addr2), (frames2, _)) in &self.sessions {
                            if peer_addr != peer_addr2 {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} forwarding control frame to {:?}<->{:?}: {:?}",
                                    local_addr,
                                    peer_addr,
                                    local_addr2,
                                    peer_addr2,
                                    frame,
                                );
                                frames2
                                    .sender
                                    .send(ProtocolFrame::Control(frame.clone()))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting session {:?}<->{:?} frame sender error: {err}",
                                            local_addr2, peer_addr2
                                        )
                                    })?;
                            }
                        }

                        match frame {
                            ProtocolControlFrame::CreatePeer(peer_id, peer_role_id) => {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} got create peer {:?} with role {:?}",
                                    local_addr,
                                    peer_addr,
                                    peer_id,
                                    peer_role_id,
                                );
                                self.engine_event
                                    .sender
                                    .send(MeetingEngineEvent::PeerJoined(peer_id, peer_role_id))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting session {:?}<->{:?} outside engine sender error: {err}",
                                            local_addr,
                                            peer_addr
                                        )
                                    })
                                    .unwrap();
                            }
                            ProtocolControlFrame::DestroyPeer(peer_id) => {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} got destroy peer {:?}",
                                    local_addr,
                                    peer_addr,
                                    peer_id,
                                );
                                self.engine_event
                                    .sender
                                    .send(MeetingEngineEvent::PeerLeft(peer_id))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting session {:?}<->{:?} outside engine sender error: {err}",
                                            local_addr,
                                            peer_addr
                                        )
                                    })
                                    .unwrap();
                            }
                            _ => {
                                if self.config.warn_unhandled_control_frame {
                                    tracing::event!(
                                        target: "tehuti::socket::session",
                                        tracing::Level::WARN,
                                        "Meeting session {:?}<->{:?} got unhandled control frame: {:?}",
                                        local_addr,
                                        peer_addr,
                                        frame,
                                    );
                                }
                            }
                        }
                    }
                    ProtocolFrame::Packet(frame) => {
                        for ((local_addr2, peer_addr2), (frames2, engine_id2)) in &self.sessions {
                            if !frame.data.recepients.is_empty()
                                && !frame.data.recepients.contains(engine_id2)
                            {
                                continue;
                            }
                            if peer_addr != peer_addr2 {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} forwarding packet frame for peer {:?} channel {:?} to {:?}<->{:?}. {} bytes",
                                    local_addr,
                                    peer_addr,
                                    frame.peer_id,
                                    frame.channel_id,
                                    local_addr2,
                                    peer_addr2,
                                    frame.data.data.len(),
                                );
                                frames2
                                    .sender
                                    .send(ProtocolFrame::Packet(frame.clone()))
                                    .map_err(|err| {
                                        format!(
                                            "Meeting session {:?}<->{:?} frame sender error: {err}",
                                            local_addr2, peer_addr2
                                        )
                                    })?;
                            }
                        }

                        if let Some(peer) = self.peers.get(&frame.peer_id) {
                            if let Some(sender) = peer.packet_senders.get(&frame.channel_id) {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} got packet frame for peer {:?} channel {:?}: {} bytes",
                                    local_addr,
                                    peer_addr,
                                    frame.peer_id,
                                    frame.channel_id,
                                    frame.data.data.len(),
                                );
                                sender
                                    .sender
                                    .send(frame.data)
                                    .map_err(|err| {
                                        format!(
                                            "Meeting session {:?}<->{:?} packet sender error: {err}",
                                            local_addr,
                                            peer_addr
                                        )
                                    })
                                    .unwrap();
                            } else if self.config.warn_unknown_channel_packet {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::WARN,
                                    "Meeting session {:?}<->{:?} got packet frame for unknown channel {:?} of peer {:?}",
                                    local_addr,
                                    peer_addr,
                                    frame.channel_id,
                                    frame.peer_id,
                                );
                            }
                        } else if self.config.warn_unknown_peer_packet {
                            tracing::event!(
                                target: "tehuti::socket::session",
                                tracing::Level::WARN,
                                "Meeting session {:?}<->{:?} got packet frame for unknown peer {:?}",
                                local_addr,
                                peer_addr,
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
                    for ((local_addr, peer_addr), (frames, engine_id)) in &self.sessions {
                        if !recepients.is_empty() && !recepients.contains(engine_id) {
                            continue;
                        }
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Meeting session {:?}<->{:?} sending packet frame for peer {:?} channel {:?}. {} bytes",
                            local_addr,
                            peer_addr,
                            peer.info.peer_id,
                            channel_id,
                            size,
                        );
                        frames.sender.send(frame.clone()).map_err(|err| {
                            format!(
                                "Meeting session {:?}<->{:?} frame sender error: {}",
                                local_addr, peer_addr, err
                            )
                        })?;
                    }
                }
            }
        }

        // Handle meeting pump.
        if let Err(err) = self.meeting.pump_all() {
            tracing::event!(
                target: "tehuti::socket::session",
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
                        target: "tehuti::socket::session",
                        tracing::Level::TRACE,
                        "Meeting got destroyed. Terminating",
                    );
                    return Err("Meeting destroyed".into());
                }
                MeetingEngineEvent::PeerCreated(descriptor) => {
                    if let Entry::Vacant(entry) = self.peers.entry(descriptor.info.peer_id) {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Meeting created peer {:?}",
                            descriptor.info.peer_id,
                        );
                        if !descriptor.info.remote {
                            let frame = ProtocolFrame::Control(ProtocolControlFrame::CreatePeer(
                                descriptor.info.peer_id,
                                descriptor.info.role_id,
                            ));
                            for ((local_addr, peer_addr), (frames, _)) in &self.sessions {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} sending create peer {:?} with role {:?}",
                                    local_addr,
                                    peer_addr,
                                    descriptor.info.peer_id,
                                    descriptor.info.role_id,
                                );
                                frames.sender.send(frame.clone()).map_err(|err| {
                                    format!(
                                        "Meeting session {:?}<->{:?} frame sender error: {}",
                                        local_addr, peer_addr, err
                                    )
                                })?;
                            }
                        }
                        entry.insert(descriptor);
                    } else if self.config.warn_duplicate_peer_creation {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Meeting tries to create duplicate peer {:?} created",
                            descriptor.info.peer_id,
                        );
                    }
                }
                MeetingEngineEvent::PeerDestroyed(peer_id) => {
                    if self.peers.contains_key(&peer_id) {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Meeting destroyed peer {:?}",
                            peer_id,
                        );
                        let frame =
                            ProtocolFrame::Control(ProtocolControlFrame::DestroyPeer(peer_id));
                        for ((local_addr, peer_addr), (frames, _)) in &self.sessions {
                            tracing::event!(
                                target: "tehuti::socket::session",
                                tracing::Level::TRACE,
                                "Meeting session {:?}<->{:?} sending destroy peer {:?}",
                                local_addr,
                                peer_addr,
                                peer_id,
                            );
                            frames.sender.send(frame.clone()).map_err(|err| {
                                format!(
                                    "Meeting session {:?}<->{:?} frame sender error: {}",
                                    local_addr, peer_addr, err
                                )
                            })?;
                        }
                        self.peers.remove(&peer_id);
                    } else if self.config.warn_unknown_peer_destruction {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Meeting tries to destroy unknown peer {:?} destroyed",
                            peer_id,
                        );
                    }
                }
                event => {
                    if self.config.warn_unhandled_engine_event {
                        tracing::event!(
                            target: "tehuti::socket::session",
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

    pub fn run(
        mut self,
        interval: Duration,
        terminate_receiver: Receiver<()>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        Ok(Builder::new().name("Meeting".to_string()).spawn(move || {
            loop {
                if terminate_receiver.try_recv().is_some() {
                    tracing::event!(
                        target: "tehuti::socket::meeting",
                        tracing::Level::TRACE,
                        "Meeting terminating on request",
                    );
                    break;
                }
                if let Err(err) = self.maintain() {
                    tracing::event!(
                        target: "tehuti::socket::meeting",
                        tracing::Level::ERROR,
                        "Meeting terminated with error: {}",
                        err,
                    );
                    return;
                }
                sleep(interval);
            }
        })?)
    }

    pub async fn into_future(mut self) -> Result<(), Box<dyn Error>> {
        loop {
            if let Err(err) = self.maintain() {
                tracing::event!(
                    target: "tehuti::socket::meeting",
                    tracing::Level::ERROR,
                    "Meeting terminated with error: {}",
                    err,
                );
                break;
            }
            pending::<()>().await;
        }
        Ok(())
    }
}

pub struct TcpHostSessionEvent {
    pub local_addr: SocketAddr,
    pub peer_addr: SocketAddr,
    pub engine_id: EngineId,
    pub frames: Duplex<ProtocolFrame>,
    pub terminate_sender: Sender<()>,
}

pub struct TcpHost {
    listener: TcpListener,
    local_engine_id: EngineId,
}

impl TcpHost {
    pub fn new(listener: TcpListener, local_engine_id: EngineId) -> Result<Self, Box<dyn Error>> {
        listener.set_nonblocking(true)?;
        tracing::event!(
            target: "tehuti::socket::host",
            tracing::Level::TRACE,
            "TcpHost created. Local address: {:?}, local engine ID: {:?}",
            listener.local_addr()?,
            local_engine_id
        );
        Ok(TcpHost {
            listener,
            local_engine_id,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.listener.local_addr()?)
    }

    pub fn local_engine_id(&self) -> EngineId {
        self.local_engine_id
    }

    pub fn accept(&self) -> Result<Option<TcpSessionResult>, Box<dyn Error>> {
        match self.listener.accept() {
            Ok((stream, _)) => match TcpSession::make(stream, self.local_engine_id) {
                Ok(session_result) => {
                    return Ok(Some(session_result));
                }
                Err(err) => {
                    tracing::event!(
                        target: "tehuti::socket::host",
                        tracing::Level::ERROR,
                        "Failed to create session: {}",
                        err,
                    );
                }
            },
            Err(ref e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(err) => {
                tracing::event!(
                    target: "tehuti::socket::host",
                    tracing::Level::ERROR,
                    "Failed to accept incoming connection: {}",
                    err,
                );
            }
        }
        Ok(None)
    }

    pub async fn accept_async(&self) -> Result<TcpSessionResult, Box<dyn Error>> {
        loop {
            match self.accept() {
                Ok(Some(session_result)) => {
                    return Ok(session_result);
                }
                Ok(None) => {
                    pending::<()>().await;
                }
                Err(err) => {
                    tracing::event!(
                        target: "tehuti::socket::host",
                        tracing::Level::ERROR,
                        "Session listener encountered error: {}",
                        err,
                    );
                    return Err(err);
                }
            }
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn run(
        self,
        interval: Duration,
        session_interval: Duration,
        session_sender: Sender<TcpHostSessionEvent>,
        terminate_receiver: Receiver<()>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        Ok(Builder::new()
            .name("Host Listener".to_string())
            .spawn(move || {
                loop {
                    if terminate_receiver.try_recv().is_some() {
                        tracing::event!(
                            target: "tehuti::socket::host",
                            tracing::Level::TRACE,
                            "Host {:?} terminating on request",
                            self.listener.local_addr().unwrap()
                        );
                        break;
                    }
                    match self.accept() {
                        Ok(Some(session_result)) => {
                            let (terminate_sender, terminate_receiver) = unbounded();
                            let TcpSessionResult { session, frames } = session_result;
                            if let Err(err) = session_sender.send(TcpHostSessionEvent {
                                local_addr: session.local_addr().unwrap(),
                                peer_addr: session.peer_addr().unwrap(),
                                engine_id: session.remote_engine_id(),
                                frames,
                                terminate_sender,
                            }) {
                                tracing::event!(
                                    target: "tehuti::socket::host",
                                    tracing::Level::ERROR,
                                    "Failed to send created session frames: {}",
                                    err,
                                );
                            }
                            if let Err(err) = session.run(session_interval, terminate_receiver) {
                                tracing::event!(
                                    target: "tehuti::socket::host",
                                    tracing::Level::ERROR,
                                    "Failed to run session: {}",
                                    err,
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(err) => {
                            tracing::event!(
                                target: "tehuti::socket::host",
                                tracing::Level::ERROR,
                                "Session listener encountered error: {}",
                                err,
                            );
                        }
                    }
                    sleep(interval);
                }
            })?)
    }
}

pub struct TcpSessionResult {
    pub session: TcpSession,
    pub frames: Duplex<ProtocolFrame>,
}

pub struct TcpSession {
    stream: TcpStream,
    local_engine_id: EngineId,
    remote_engine_id: EngineId,
    buffer_in: Vec<u8>,
    buffer_out: Vec<u8>,
    frames: Duplex<ProtocolFrame>,
    terminated: bool,
}

impl TcpSession {
    pub fn make(
        mut stream: TcpStream,
        local_engine_id: EngineId,
    ) -> Result<TcpSessionResult, Box<dyn Error>> {
        stream.set_nodelay(true)?;
        stream.set_nonblocking(true)?;
        stream.write_all(&local_engine_id.id().to_le_bytes())?;
        let mut remote_engine_id_bytes = [0u8; std::mem::size_of::<u128>()];
        stream.flush()?;
        loop {
            match stream.read_exact(&mut remote_engine_id_bytes) {
                Ok(()) => break,
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => {
                    return Err(format!(
                        "Session {:?}<->{:?} stream hyandshake read error: {}",
                        stream.local_addr().unwrap(),
                        stream.peer_addr().unwrap(),
                        e
                    )
                    .into());
                }
            }
        }
        let remote_engine_id = EngineId::new(u128::from_le_bytes(remote_engine_id_bytes));
        let (frames_inside, frames_outside) = Duplex::crossing_unbounded();
        tracing::event!(
            target: "tehuti::socket::session",
            tracing::Level::TRACE,
            "Session created. Local address: {:?}, peer address: {:?}, local engine ID: {:?}, remote engine ID: {:?}",
            stream.local_addr()?,
            stream.peer_addr()?,
            local_engine_id,
            remote_engine_id,
        );
        Ok(TcpSessionResult {
            session: Self {
                stream,
                local_engine_id,
                remote_engine_id,
                buffer_in: Default::default(),
                buffer_out: Default::default(),
                frames: frames_inside,
                terminated: false,
            },
            frames: frames_outside,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.stream.local_addr()?)
    }

    pub fn peer_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.stream.peer_addr()?)
    }

    pub fn local_engine_id(&self) -> EngineId {
        self.local_engine_id
    }

    pub fn remote_engine_id(&self) -> EngineId {
        self.remote_engine_id
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        if self.terminated {
            return Err(format!(
                "Session {:?}<->{:?} is terminated",
                self.stream.local_addr().unwrap(),
                self.stream.peer_addr().unwrap()
            )
            .into());
        }
        self.receive_frames()?;
        self.send_frames()?;
        Ok(())
    }

    fn receive_frames(&mut self) -> Result<(), Box<dyn Error>> {
        let mut buffer = vec![0u8; 4096];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    self.buffer_in.extend_from_slice(&buffer[..n]);
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    return Err(format!(
                        "Session {:?}<->{:?} stream read error: {}",
                        self.stream.local_addr().unwrap(),
                        self.stream.peer_addr().unwrap(),
                        e
                    )
                    .into());
                }
            }
        }
        let mut cursor = Cursor::new(&self.buffer_in);
        let mut frames = Vec::new();
        loop {
            match ProtocolFrame::read(&mut cursor) {
                Ok(frame) => frames.push(frame),
                Err(ref e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(Box::new(e)),
            }
        }
        let pos = cursor.position() as usize;
        self.buffer_in.drain(0..pos);
        for mut frame in frames {
            if let ProtocolFrame::Packet(frame) = &mut frame
                && frame.data.sender.is_none()
            {
                frame.data.sender = Some(self.remote_engine_id);
            }
            self.frames.sender.send(frame).map_err(|err| {
                format!(
                    "Session {:?}<->{:?} frame sender error: {}",
                    self.stream.local_addr().unwrap(),
                    self.stream.peer_addr().unwrap(),
                    err
                )
            })?;
        }
        Ok(())
    }

    fn send_frames(&mut self) -> Result<(), Box<dyn Error>> {
        for mut frame in self.frames.receiver.iter() {
            if let ProtocolFrame::Packet(frame) = &mut frame {
                frame.data.sender = Some(self.local_engine_id);
            }
            frame.write(&mut self.buffer_out)?;
        }
        loop {
            match self.stream.write(&self.buffer_out) {
                Ok(0) => break,
                Ok(n) => {
                    self.buffer_out.drain(0..n);
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    return Err(format!(
                        "Session {:?}<->{:?} stream write error: {}",
                        self.stream.local_addr().unwrap(),
                        self.stream.peer_addr().unwrap(),
                        e
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    pub fn run(
        mut self,
        interval: Duration,
        terminate_receiver: Receiver<()>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        Ok(Builder::new()
            .name(format!(
                "Session {:?}<->{:?}",
                self.stream.local_addr().unwrap(),
                self.stream.peer_addr().unwrap()
            ))
            .spawn(move || {
                loop {
                    if terminate_receiver.try_recv().is_some() {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Session {:?}<->{:?} terminating on request",
                            self.stream.local_addr().unwrap(),
                            self.stream.peer_addr().unwrap(),
                        );
                        break;
                    }
                    if let Err(err) = self.maintain() {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::ERROR,
                            "Session {:?}<->{:?} terminated with error: {}",
                            self.stream.local_addr().unwrap(),
                            self.stream.peer_addr().unwrap(),
                            err,
                        );
                        break;
                    }
                    sleep(interval);
                }
            })?)
    }

    pub async fn into_future(mut self) -> Result<(), Box<dyn Error>> {
        loop {
            if let Err(err) = self.maintain() {
                tracing::event!(
                    target: "tehuti::socket::session",
                    tracing::Level::ERROR,
                    "Session {:?}<->{:?} terminated with error: {}",
                    self.stream.local_addr().unwrap(),
                    self.stream.peer_addr().unwrap(),
                    err,
                );
                break;
            }
            pending::<()>().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::spawn;
    use tehuti::{
        channel::{ChannelId, ChannelMode, Dispatch},
        codec::replicable::RepCodec,
        meeting::MeetingUserEvent,
        peer::{PeerBuilder, PeerDestructurer, PeerRoleId, TypedPeer, TypedPeerRole},
    };
    use tehuti_mock::{mock_env_tracing, mock_recv_matching};
    use tracing::level_filters::LevelFilter;

    struct Chatter {
        pub messages: Duplex<Dispatch<String>>,
    }

    impl TypedPeerRole for Chatter {
        const ROLE_ID: PeerRoleId = PeerRoleId::new(0);
    }

    impl TypedPeer for Chatter {
        fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
            Ok(builder.bind_read_write::<RepCodec<String>, String>(
                ChannelId::new(0),
                ChannelMode::ReliableOrdered,
                None,
            ))
        }

        fn into_typed(mut destructurer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                messages: destructurer.read_write::<String>(ChannelId::new(0))?,
            })
        }
    }

    #[test]
    fn test_tcp_connection() {
        mock_env_tracing(LevelFilter::TRACE);

        let server_thread = spawn(|| {
            let factory =
                Arc::new(PeerFactory::default().with(PeerRoleId::new(0), Chatter::builder));
            let TcpMeetingResult {
                meeting,
                interface,
                events_sender,
            } = TcpMeeting::make("Server", TcpMeetingConfig::enable_all(), factory);
            let (terminate_meeting_sender, terminate_meeting_receiver) = unbounded();
            let meeting_thread = meeting
                .run(Duration::ZERO, terminate_meeting_receiver)
                .unwrap();

            let listener = TcpListener::bind("127.0.0.1:8888").unwrap();
            println!("* Server listening on {:?}", listener.local_addr().unwrap());
            let (session_sender, session_receiver) = unbounded();
            let (terminate_host_sender, terminate_host_receiver) = unbounded();
            let host_engine_id = EngineId::uuid();
            let host_thread = TcpHost::new(listener, host_engine_id)
                .unwrap()
                .run(
                    Duration::ZERO,
                    Duration::ZERO,
                    session_sender,
                    terminate_host_receiver,
                )
                .unwrap();

            let TcpHostSessionEvent {
                local_addr,
                peer_addr,
                engine_id,
                frames,
                terminate_sender: terminate_session_sender,
            } = session_receiver.recv_blocking().unwrap();
            println!("* Server accepted connection from {:?}", peer_addr);
            events_sender
                .send(TcpMeetingEvent::RegisterSession {
                    local_addr,
                    peer_addr,
                    engine_id,
                    frames,
                })
                .unwrap();

            println!("* Server creating peer...");
            interface
                .sender
                .send(MeetingUserEvent::PeerCreate(
                    PeerId::new(0),
                    PeerRoleId::new(0),
                ))
                .unwrap();

            println!("* Server waiting for peer to be created...");
            let peer = mock_recv_matching!(
                interface.receiver,
                Duration::from_secs(1),
                MeetingUserEvent::PeerAdded(peer) => peer
            )
            .into_typed::<Chatter>()
            .unwrap();

            sleep(Duration::from_millis(100));

            println!("* Server sending message to client...");
            peer.messages
                .sender
                .send("Hello from server to client".to_owned().into())
                .unwrap();

            sleep(Duration::from_secs(1));

            println!("* Server done, exiting...");
            drop(peer);
            terminate_session_sender.send(()).unwrap();
            terminate_meeting_sender.send(()).unwrap();
            meeting_thread.join().unwrap();
            terminate_host_sender.send(()).unwrap();
            host_thread.join().unwrap();
        });

        let client_thread = spawn(move || {
            sleep(Duration::from_millis(100));

            let factory =
                Arc::new(PeerFactory::default().with(PeerRoleId::new(0), Chatter::builder));
            let TcpMeetingResult {
                meeting,
                interface,
                events_sender,
            } = TcpMeeting::make("Client", TcpMeetingConfig::enable_all(), factory);
            let (terminate_meeting_sender, terminate_meeting_receiver) = unbounded();
            let meeting_thread = meeting
                .run(Duration::ZERO, terminate_meeting_receiver)
                .unwrap();

            let stream = TcpStream::connect("127.0.0.1:8888").unwrap();
            println!(
                "* Client connected from {:?} to {:?}",
                stream.local_addr().unwrap(),
                stream.peer_addr().unwrap()
            );

            let session_engine_id = EngineId::uuid();
            let TcpSessionResult { session, frames } =
                TcpSession::make(stream, session_engine_id).unwrap();
            events_sender
                .send(TcpMeetingEvent::RegisterSession {
                    local_addr: session.local_addr().unwrap(),
                    peer_addr: session.peer_addr().unwrap(),
                    engine_id: session_engine_id,
                    frames,
                })
                .unwrap();
            let (terminate_session_sender, terminate_session_receiver) = unbounded();
            let session_thread = session
                .run(Duration::ZERO, terminate_session_receiver)
                .unwrap();

            println!("* Client waiting for peer to be created...");
            let peer = mock_recv_matching!(
                interface.receiver,
                Duration::from_secs(1),
                MeetingUserEvent::PeerAdded(peer) => peer
            )
            .into_typed::<Chatter>()
            .unwrap();

            println!("* Client waiting for message from server...");
            let msg = peer
                .messages
                .receiver
                .recv_blocking_timeout(Duration::from_secs(1))
                .unwrap();
            println!("* Client received message: {}", msg.message);
            assert_eq!(&msg.message, "Hello from server to client");

            sleep(Duration::from_secs(1));

            println!("* Client done, exiting...");
            drop(peer);
            terminate_session_sender.send(()).unwrap();
            session_thread.join().unwrap();
            terminate_meeting_sender.send(()).unwrap();
            meeting_thread.join().unwrap();
        });

        client_thread.join().unwrap();
        server_thread.join().unwrap();
    }
}
