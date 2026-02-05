use std::{
    error::Error,
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::Arc,
    thread::spawn,
    time::Duration,
};
use tehuti::{event::unbounded, meeting::MeetingInterface, peer::PeerFactory};
use tehuti_socket::{
    TcpHost, TcpMeeting, TcpMeetingEvent, TcpMeetingResult, TcpSession, TcpSessionResult,
};

pub fn tcp_example(
    is_server: bool,
    address: impl ToSocketAddrs,
    factory: Arc<PeerFactory>,
    body: impl FnOnce(bool, MeetingInterface, SocketAddr) -> Result<(), Box<dyn Error>> + 'static,
) -> Result<(), Box<dyn Error>> {
    if is_server {
        tcp_example_server(address, factory, body)
    } else {
        tcp_example_client(address, factory, body)
    }
}

pub fn tcp_example_server(
    address: impl ToSocketAddrs,
    factory: Arc<PeerFactory>,
    body: impl FnOnce(bool, MeetingInterface, SocketAddr) -> Result<(), Box<dyn Error>> + 'static,
) -> Result<(), Box<dyn Error>> {
    let address = address.to_socket_addrs()?.next().unwrap();
    println!("* Starting server at {}", address);

    let TcpMeetingResult {
        meeting,
        interface,
        events_sender,
    } = TcpMeeting::make(address, factory);
    let (terminate_meeting_sender, terminate_meeting_receiver) = unbounded();
    let meeting_thread = meeting
        .run(Duration::ZERO, terminate_meeting_receiver)
        .unwrap();

    let listener = TcpListener::bind(address)?;
    let local_addr = listener.local_addr()?;
    println!("* Server listening at: {}", local_addr);
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

    let (terminate_session_sender, terminate_session_receiver) = unbounded();
    let sessions_pump = spawn(move || {
        let mut sessions = Vec::new();
        loop {
            if terminate_session_receiver.try_recv().is_some() {
                break;
            }
            if let Some((local_addr, peer_addr, frames, terminate_session_sender)) =
                session_receiver.try_recv()
            {
                println!("* Accepted connection from {}", peer_addr);
                events_sender
                    .send(TcpMeetingEvent::RegisterSession {
                        local_addr,
                        peer_addr,
                        frames,
                    })
                    .unwrap();
                sessions.push(terminate_session_sender);
            }
        }
        for terminate_session_sender in sessions {
            let _ = terminate_session_sender.send(());
        }
    });

    body(true, interface, local_addr)?;
    terminate_session_sender.send(())?;
    sessions_pump.join().unwrap();
    terminate_host_sender.send(())?;
    host_thread.join().unwrap();
    terminate_meeting_sender.send(())?;
    meeting_thread.join().unwrap();
    Ok(())
}

pub fn tcp_example_client(
    address: impl ToSocketAddrs,
    factory: Arc<PeerFactory>,
    body: impl FnOnce(bool, MeetingInterface, SocketAddr) -> Result<(), Box<dyn Error>> + 'static,
) -> Result<(), Box<dyn Error>> {
    let address = address.to_socket_addrs()?.next().unwrap();
    println!("* Connecting to server at {}", address);

    let TcpMeetingResult {
        meeting,
        interface,
        events_sender,
    } = TcpMeeting::make(address, factory);
    let (terminate_meeting_sender, terminate_meeting_receiver) = unbounded();
    let meeting_thread = meeting
        .run(Duration::ZERO, terminate_meeting_receiver)
        .unwrap();

    let stream = TcpStream::connect(address)?;
    let local_addr = stream.local_addr()?;
    println!("* Client connected at: {}", local_addr);
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

    body(false, interface, local_addr)?;
    terminate_session_sender.send(()).unwrap();
    session_thread.join().unwrap();
    terminate_meeting_sender.send(()).unwrap();
    meeting_thread.join().unwrap();
    Ok(())
}
