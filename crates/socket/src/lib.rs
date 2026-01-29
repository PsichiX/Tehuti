use std::{
    collections::{BTreeMap, btree_map::Entry},
    error::Error,
    future::pending,
    io::{Cursor, ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    thread::{Builder, JoinHandle, sleep},
    time::Duration,
    vec,
};
use tehuti::{
    engine::EnginePeerDescriptor,
    event::{Duplex, Receiver, Sender, unbounded},
    meeting::{Meeting, MeetingEngineEvent, MeetingInterface, MeetingInterfaceResult},
    peer::{PeerFactory, PeerId},
    protocol::{ProtocolControlFrame, ProtocolFrame, ProtocolPacketFrame},
};

pub struct TcpHost {
    listener: TcpListener,
    factory: Arc<PeerFactory>,
}

impl TcpHost {
    pub fn make(listener: TcpListener, factory: Arc<PeerFactory>) -> Result<Self, Box<dyn Error>> {
        listener.set_nonblocking(true)?;
        Ok(TcpHost { listener, factory })
    }

    pub fn accept(&self) -> Result<Option<TcpSessionResult>, Box<dyn Error>> {
        match self.listener.accept() {
            Ok((stream, _)) => match TcpSession::make(stream, self.factory.clone()) {
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
        meeting_sender: Sender<(MeetingInterface, Sender<()>)>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        Ok(Builder::new()
            .name("Session Listener".to_string())
            .spawn(move || {
                loop {
                    match self.accept() {
                        Ok(Some(session_result)) => {
                            let (terminate_sender, terminate_receiver) = unbounded();
                            let TcpSessionResult { session, interface } = session_result;
                            if let Err(err) = meeting_sender.send((interface, terminate_sender)) {
                                tracing::event!(
                                    target: "tehuti::socket::host",
                                    tracing::Level::ERROR,
                                    "Failed to send meeting interface to engine: {}",
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
    pub interface: MeetingInterface,
}

pub struct TcpSession {
    id: String,
    stream: TcpStream,
    meeting: Meeting,
    engine_event: Duplex<MeetingEngineEvent>,
    peers: BTreeMap<PeerId, EnginePeerDescriptor>,
    buffer_in: Vec<u8>,
    buffer_out: Vec<u8>,
    terminated: bool,
}

impl TcpSession {
    pub fn make(
        stream: TcpStream,
        factory: Arc<PeerFactory>,
    ) -> Result<TcpSessionResult, Box<dyn Error>> {
        let id = format!("{}<->{}", stream.local_addr()?, stream.peer_addr()?);
        let MeetingInterfaceResult {
            meeting,
            interface,
            engine_event,
        } = MeetingInterface::make(factory, id.clone());
        stream.set_nonblocking(true)?;
        Ok(TcpSessionResult {
            session: TcpSession {
                id,
                stream,
                meeting,
                engine_event,
                peers: Default::default(),
                buffer_in: Default::default(),
                buffer_out: Default::default(),
                terminated: false,
            },
            interface,
        })
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        if self.terminated {
            return Err(format!("Session {} is terminated", self.id).into());
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
            match frame {
                ProtocolFrame::Control(frame) => match frame {
                    ProtocolControlFrame::CreatePeer(peer_id, peer_role_id) => {
                        self.engine_event
                            .sender
                            .send(MeetingEngineEvent::PeerJoined(peer_id, peer_role_id))
                            .map_err(|err| {
                                format!("Session {} outside engine sender error: {err}", self.id)
                            })
                            .unwrap();
                    }
                    ProtocolControlFrame::DestroyPeer(peer_id) => {
                        self.engine_event
                            .sender
                            .send(MeetingEngineEvent::PeerLeft(peer_id))
                            .map_err(|err| {
                                format!("Session {} outside engine sender error: {err}", self.id)
                            })
                            .unwrap();
                    }
                    _ => {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Session {} got unhandled control frame: {:?}",
                            self.id,
                            frame,
                        );
                    }
                },
                ProtocolFrame::Packet(frame) => {
                    if let Some(peer) = self.peers.get(&frame.peer_id) {
                        if let Some(sender) = peer.packet_senders.get(&frame.channel_id) {
                            sender
                                .sender
                                .send(frame.data)
                                .map_err(|err| {
                                    format!("Session {} packet sender error: {err}", self.id)
                                })
                                .unwrap();
                        } else {
                            tracing::event!(
                                target: "tehuti::socket::session",
                                tracing::Level::WARN,
                                "Session {} got packet frame for unknown channel {:?} of peer {:?}",
                                self.id,
                                frame.channel_id,
                                frame.peer_id,
                            );
                        }
                    } else {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Session {} got packet frame for unknown peer {:?}",
                            self.id,
                            frame.peer_id,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn send_frames(&mut self) -> Result<(), Box<dyn Error>> {
        for peer in self.peers.values() {
            for (channel_id, receiver) in &peer.packet_receivers {
                for data in receiver.receiver.iter() {
                    ProtocolFrame::Packet(ProtocolPacketFrame {
                        peer_id: peer.info.peer_id,
                        channel_id: *channel_id,
                        data,
                    })
                    .write(&mut self.buffer_out)?;
                }
            }
        }
        if let Err(err) = self.meeting.pump_all() {
            tracing::event!(
                target: "tehuti::socket::session",
                tracing::Level::ERROR,
                "Session {} encountered error: {}. Terminating",
                self.id,
                err,
            );
            self.terminated = true;
            return Err(err);
        }
        for event in self.engine_event.receiver.iter() {
            match event {
                MeetingEngineEvent::MeetingDestroyed => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::TRACE,
                        "Session {} terminating",
                        self.id,
                    );
                    self.terminated = true;
                    return Err(format!("Session {} meeting destroyed", self.id).into());
                }
                MeetingEngineEvent::PeerCreated(descriptor) => {
                    if let Entry::Vacant(entry) = self.peers.entry(descriptor.info.peer_id) {
                        if !descriptor.info.remote {
                            ProtocolFrame::Control(ProtocolControlFrame::CreatePeer(
                                descriptor.info.peer_id,
                                descriptor.info.role_id,
                            ))
                            .write(&mut self.buffer_out)?;
                        }
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Session {} created peer {:?}",
                            self.id,
                            descriptor.info.peer_id,
                        );
                        entry.insert(descriptor);
                    } else {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Session {} got duplicate peer {:?} created",
                            self.id,
                            descriptor.info.peer_id,
                        );
                    }
                }
                MeetingEngineEvent::PeerDestroyed(peer_id) => {
                    if self.peers.contains_key(&peer_id) {
                        ProtocolFrame::Control(ProtocolControlFrame::DestroyPeer(peer_id))
                            .write(&mut self.buffer_out)?;
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Session {} destroyed peer {:?}",
                            self.id,
                            peer_id,
                        );
                        self.peers.remove(&peer_id);
                    } else {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::WARN,
                            "Session {} got unknown peer {:?} destroyed",
                            self.id,
                            peer_id,
                        );
                    }
                }
                event => {
                    tracing::event!(
                        target: "tehuti::socket::session",
                        tracing::Level::WARN,
                        "Session {} got unhandled engine event: {:?}",
                        self.id,
                        event,
                    );
                }
            }
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
            .name(format!("Session {}", self.id))
            .spawn(move || {
                loop {
                    if terminate_receiver.try_recv().is_some() {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::TRACE,
                            "Session {} terminating on request",
                            self.id,
                        );
                        break;
                    }
                    if let Err(err) = self.maintain() {
                        tracing::event!(
                            target: "tehuti::socket::session",
                            tracing::Level::ERROR,
                            "Session {} terminated with error: {}",
                            self.id,
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
                    "Session {} terminated with error: {}",
                    self.id,
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
    use tehuti::{
        channel::{ChannelId, ChannelMode},
        meeting::MeetingUserEvent,
        peer::{PeerBuilder, PeerDestructurer, PeerRoleId, TypedPeer},
    };
    use tehuti_mock::{mock_env_tracing, mock_recv_matching};

    struct Chatter {
        pub sender: Sender<String>,
        pub receiver: Receiver<String>,
    }

    impl TypedPeer for Chatter {
        fn builder(builder: PeerBuilder) -> PeerBuilder {
            builder.bind_read_write::<String, String>(
                ChannelId::new(0),
                ChannelMode::ReliableOrdered,
                None,
            )
        }

        fn into_typed(mut destructurer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
            let sender = destructurer.write::<String>(ChannelId::new(0))?;
            let receiver = destructurer.read::<String>(ChannelId::new(0))?;
            Ok(Self { sender, receiver })
        }
    }

    #[test]
    fn test_tcp_session_creation() {
        mock_env_tracing();

        let factory = Arc::new(PeerFactory::default().with(PeerRoleId::new(0), Chatter::builder));

        let listener = TcpListener::bind("127.0.0.1:8888").unwrap();
        let host = TcpHost::make(listener, factory.clone()).unwrap();
        let stream = TcpStream::connect("127.0.0.1:8888").unwrap();

        let TcpSessionResult {
            session,
            interface: meeting_client,
        } = TcpSession::make(stream, factory).unwrap();
        let (terminate_client, terminate_receiver) = unbounded();
        let session_client = session.run(Duration::ZERO, terminate_receiver).unwrap();

        sleep(Duration::from_millis(100));

        let TcpSessionResult {
            session,
            interface: meeting_server,
        } = host.accept().unwrap().unwrap();
        let (terminate_server, terminate_receiver) = unbounded();
        let session_server = session.run(Duration::ZERO, terminate_receiver).unwrap();

        meeting_server
            .sender
            .send(MeetingUserEvent::PeerCreate(
                PeerId::new(0),
                PeerRoleId::new(0),
            ))
            .unwrap();

        let peer_server = mock_recv_matching!(
            meeting_server.receiver,
            Duration::from_secs(1),
            MeetingUserEvent::PeerAdded(peer) => peer
        )
        .into_typed::<Chatter>()
        .unwrap();

        let peer_client = mock_recv_matching!(
            meeting_client.receiver,
            Duration::from_secs(1),
            MeetingUserEvent::PeerAdded(peer) => peer
        )
        .into_typed::<Chatter>()
        .unwrap();

        peer_server
            .sender
            .send("Hello from server to client".to_owned())
            .unwrap();

        let msg = peer_client
            .receiver
            .recv_blocking_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(&msg, "Hello from server to client");

        terminate_client.send(()).unwrap();
        terminate_server.send(()).unwrap();
        session_client.join().unwrap();
        session_server.join().unwrap();
    }
}
