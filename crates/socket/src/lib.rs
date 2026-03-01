use std::{
    error::Error,
    future::pending,
    io::{Cursor, ErrorKind, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread::{Builder, JoinHandle, sleep},
    time::Duration,
    vec,
};
use tehuti::{
    engine::{
        EngineId, EngineMeeting, EngineMeetingConfig, EngineMeetingEvent, EngineMeetingResult,
    },
    event::{Duplex, Receiver, Sender, unbounded},
    protocol::ProtocolFrame,
};

pub type TcpMeetingEvent = EngineMeetingEvent;
pub type TcpMeetingResult = EngineMeetingResult;
pub type TcpMeeting = EngineMeeting;
pub type TcpMeetingConfig = EngineMeetingConfig;

pub trait TcpMeetingExt {
    fn run(
        self,
        interval: Duration,
        terminate_receiver: Receiver<()>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>>;
}

impl TcpMeetingExt for EngineMeeting {
    fn run(
        mut self,
        interval: Duration,
        terminate_receiver: Receiver<()>,
    ) -> Result<JoinHandle<()>, Box<dyn Error>> {
        Ok(Builder::new()
            .name("TcpMeeting".to_string())
            .spawn(move || {
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
    use std::{sync::Arc, thread::spawn};
    use tehuti::{
        channel::{ChannelId, ChannelMode, Dispatch},
        codec::replicable::RepCodec,
        meeting::MeetingUserEvent,
        peer::{
            PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer,
            TypedPeerRole,
        },
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
            } = TcpMeeting::make(
                "Server",
                TcpMeetingConfig::default().enable_all_warnings(),
                factory,
            );
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
                peer_addr,
                engine_id,
                frames,
                terminate_sender: terminate_session_sender,
                ..
            } = session_receiver.recv_blocking().unwrap();
            println!("* Server accepted connection from {:?}", peer_addr);
            events_sender
                .send(TcpMeetingEvent::RegisterEngine { engine_id, frames })
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
            } = TcpMeeting::make(
                "Client",
                TcpMeetingConfig::default().enable_all_warnings(),
                factory,
            );
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
                .send(TcpMeetingEvent::RegisterEngine {
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
