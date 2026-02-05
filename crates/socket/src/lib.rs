use std::{
    collections::{BTreeMap, HashMap, btree_map::Entry},
    error::Error,
    future::pending,
    io::{Cursor, ErrorKind, Read, Write},
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
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

pub fn ip_to_engine_id(addr: IpAddr) -> EngineId {
    let bits = match addr {
        IpAddr::V4(addr) => addr.to_bits() as u128,
        IpAddr::V6(addr) => addr.to_bits(),
    };
    EngineId::new(bits)
}

pub enum TcpMeetingEvent {
    RegisterSession {
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
        frames: Duplex<ProtocolFrame>,
    },
    UnregisterSession {
        local_addr: SocketAddr,
        peer_addr: SocketAddr,
    },
}

pub struct TcpMeetingResult {
    pub meeting: TcpMeeting,
    pub interface: MeetingInterface,
    pub events_sender: Sender<TcpMeetingEvent>,
}

pub struct TcpMeeting {
    meeting: Meeting,
    engine_event: Duplex<MeetingEngineEvent>,
    peers: BTreeMap<PeerId, EnginePeerDescriptor>,
    sessions: HashMap<(SocketAddr, SocketAddr), Duplex<ProtocolFrame>>,
    events_receiver: Receiver<TcpMeetingEvent>,
}

impl TcpMeeting {
    pub fn make(name: impl ToString, factory: Arc<PeerFactory>) -> TcpMeetingResult {
        let MeetingInterfaceResult {
            meeting,
            interface,
            engine_event,
        } = MeetingInterface::make(factory, name);
        let (events_sender, events_receiver) = unbounded();
        TcpMeetingResult {
            meeting: Self {
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
                    frames,
                } => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::TRACE,
                        "Meeting registering session: {:?}<->{:?}",
                        local_addr,
                        peer_addr,
                    );
                    for descriptor in self.peers.values() {
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
                    self.sessions.insert((local_addr, peer_addr), frames);
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
        for ((local_addr, peer_addr), frames) in &self.sessions {
            for frame in frames.receiver.iter() {
                match frame {
                    ProtocolFrame::Control(frame) => {
                        for ((local_addr2, peer_addr2), frames2) in &self.sessions {
                            if peer_addr != peer_addr2 {
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
                    ProtocolFrame::Packet(frame) => {
                        for ((local_addr2, peer_addr2), frames2) in &self.sessions {
                            if !frame.data.recepients.is_empty()
                                && !frame
                                    .data
                                    .recepients
                                    .contains(&ip_to_engine_id(peer_addr2.ip()))
                            {
                                continue;
                            }
                            if peer_addr != peer_addr2 {
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
                            } else {
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
                        } else {
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
                    for ((local_addr, peer_addr), frames) in &self.sessions {
                        if !recepients.is_empty()
                            && !recepients.contains(&ip_to_engine_id(peer_addr.ip()))
                        {
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
                            for ((local_addr, peer_addr), frames) in &self.sessions {
                                tracing::event!(
                                    target: "tehuti::socket::session",
                                    tracing::Level::TRACE,
                                    "Meeting session {:?}<->{:?} creating peer {:?} with role {:?}",
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
                    } else {
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
                        for ((local_addr, peer_addr), frames) in &self.sessions {
                            tracing::event!(
                                target: "tehuti::socket::session",
                                tracing::Level::TRACE,
                                "Meeting session {:?}<->{:?} destroying peer {:?}",
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
                    } else {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Meeting tries to destroy unknown peer {:?} destroyed",
                            peer_id,
                        );
                    }
                }
                event => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::WARN,
                        "Meeting got unhandled engine event: {:?}",
                        event,
                    );
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

pub struct TcpHost {
    listener: TcpListener,
}

impl TcpHost {
    pub fn new(listener: TcpListener) -> Result<Self, Box<dyn Error>> {
        listener.set_nonblocking(true)?;
        Ok(TcpHost { listener })
    }

    pub fn my_engine_id(&self) -> Result<EngineId, Box<dyn Error>> {
        Ok(ip_to_engine_id(self.local_addr()?.ip()))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.listener.local_addr()?)
    }

    pub fn accept(&self) -> Result<Option<TcpSessionResult>, Box<dyn Error>> {
        match self.listener.accept() {
            Ok((stream, _)) => match TcpSession::make(stream) {
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

    pub fn run(
        self,
        interval: Duration,
        session_interval: Duration,
        session_sender: Sender<(SocketAddr, SocketAddr, Duplex<ProtocolFrame>, Sender<()>)>,
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
                            if let Err(err) = session_sender.send((
                                session.local_addr().unwrap(),
                                session.peer_addr().unwrap(),
                                frames,
                                terminate_sender,
                            )) {
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
    buffer_in: Vec<u8>,
    buffer_out: Vec<u8>,
    frames: Duplex<ProtocolFrame>,
    terminated: bool,
}

impl TcpSession {
    pub fn make(stream: TcpStream) -> Result<TcpSessionResult, Box<dyn Error>> {
        stream.set_nonblocking(true)?;
        let (frames_inside, frames_outside) = Duplex::crossing_unbounded();
        Ok(TcpSessionResult {
            session: Self {
                stream,
                buffer_in: Default::default(),
                buffer_out: Default::default(),
                frames: frames_inside,
                terminated: false,
            },
            frames: frames_outside,
        })
    }

    pub fn my_engine_id(&self) -> Result<EngineId, Box<dyn Error>> {
        Ok(ip_to_engine_id(self.local_addr()?.ip()))
    }

    pub fn other_engine_id(&self) -> Result<EngineId, Box<dyn Error>> {
        Ok(ip_to_engine_id(self.peer_addr()?.ip()))
    }

    pub fn local_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.stream.local_addr()?)
    }

    pub fn peer_addr(&self) -> Result<SocketAddr, Box<dyn Error>> {
        Ok(self.stream.peer_addr()?)
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
                Err(e) => return Err(Box::new(e)),
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
        for frame in frames {
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
        for frame in self.frames.receiver.iter() {
            frame.write(&mut self.buffer_out)?;
        }
        loop {
            match self.stream.write(&self.buffer_out) {
                Ok(0) => break,
                Ok(n) => {
                    self.buffer_out.drain(0..n);
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => return Err(Box::new(e)),
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
        meeting::MeetingUserEvent,
        peer::{PeerBuilder, PeerDestructurer, PeerRoleId, TypedPeer},
    };
    use tehuti_mock::{mock_env_tracing, mock_recv_matching};

    struct Chatter {
        pub messages: Duplex<Dispatch<String>>,
    }

    impl TypedPeer for Chatter {
        const ROLE_ID: PeerRoleId = PeerRoleId::new(0);

        fn builder(builder: PeerBuilder) -> PeerBuilder {
            builder.bind_read_write::<String, String>(
                ChannelId::new(0),
                ChannelMode::ReliableOrdered,
                None,
            )
        }

        fn into_typed(mut destructurer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
            Ok(Self {
                messages: destructurer.read_write::<String>(ChannelId::new(0))?,
            })
        }
    }

    #[test]
    fn test_tcp_connection() {
        mock_env_tracing();

        let server_thread = spawn(|| {
            let factory =
                Arc::new(PeerFactory::default().with(PeerRoleId::new(0), Chatter::builder));
            let TcpMeetingResult {
                meeting,
                interface,
                events_sender,
            } = TcpMeeting::make("Server", factory);
            let (terminate_meeting_sender, terminate_meeting_receiver) = unbounded();
            let meeting_thread = meeting
                .run(Duration::ZERO, terminate_meeting_receiver)
                .unwrap();

            let listener = TcpListener::bind("127.0.0.1:8888").unwrap();
            println!("* Server listening on {:?}", listener.local_addr().unwrap());
            let (session_sender, session_receiver) = unbounded();
            let (terminate_host_sender, terminate_host_receiver) = unbounded();
            let host_thread = TcpHost::new(listener)
                .unwrap()
                .run(
                    Duration::ZERO,
                    Duration::ZERO,
                    session_sender,
                    terminate_host_receiver,
                )
                .unwrap();

            let (local_addr, peer_addr, frames, terminate_session_sender) =
                session_receiver.recv_blocking().unwrap();
            println!("* Server accepted connection from {:?}", peer_addr);
            events_sender
                .send(TcpMeetingEvent::RegisterSession {
                    local_addr,
                    peer_addr,
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
            } = TcpMeeting::make("Client", factory);
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

            let TcpSessionResult { session, frames } = TcpSession::make(stream).unwrap();
            events_sender
                .send(TcpMeetingEvent::RegisterSession {
                    local_addr: session.local_addr().unwrap(),
                    peer_addr: session.peer_addr().unwrap(),
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
