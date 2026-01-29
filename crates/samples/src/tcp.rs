use std::{
    error::Error,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};
use tehuti::{
    event::{Sender, unbounded},
    meeting::MeetingInterface,
    peer::PeerFactory,
};
use tehuti_socket::{TcpHost, TcpSession, TcpSessionResult};

pub fn tcp_example_server(
    address: impl ToSocketAddrs,
    factory: Arc<PeerFactory>,
    body: impl FnOnce(bool, MeetingInterface, Sender<()>) + 'static,
) -> Result<(), Box<dyn Error>> {
    println!(
        "* Starting server at {}",
        address.to_socket_addrs()?.next().unwrap()
    );

    let listener = TcpListener::bind(address)?;
    let host = TcpHost::make(listener, factory)?;
    let TcpSessionResult { session, interface } = loop {
        if let Some(session) = host.accept()? {
            break session;
        };
    };
    let (terminate_sender, terminate_receiver) = unbounded();
    let session = session.run(Duration::ZERO, terminate_receiver)?;
    body(true, interface, terminate_sender);
    session.join().unwrap();
    Ok(())
}

pub fn tcp_example_client(
    address: impl ToSocketAddrs,
    factory: Arc<PeerFactory>,
    body: impl FnOnce(bool, MeetingInterface, Sender<()>) + 'static,
) -> Result<(), Box<dyn Error>> {
    println!(
        "* Connecting to server at {}",
        address.to_socket_addrs()?.next().unwrap()
    );

    let stream = TcpStream::connect(address)?;
    let TcpSessionResult { session, interface } = TcpSession::make(stream, factory)?;
    let (terminate_sender, terminate_receiver) = unbounded();
    let session = session.run(Duration::ZERO, terminate_receiver)?;
    body(false, interface, terminate_sender);
    session.join().unwrap();
    Ok(())
}
