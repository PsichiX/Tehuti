use crate::{
    Duplex,
    channel::Channel,
    engine::EnginePeerDescriptor,
    peer::{Peer, PeerBuildResult, PeerFactory, PeerId, PeerRoleId},
};
use std::{
    collections::BTreeMap,
    error::Error,
    ops::Deref,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

#[derive(Debug)]
pub enum MeetingEngineEvent {
    /// Notification that the meeting has been destroyed.
    MeetingDestroyed,
    /// Notification that a peer has been created.
    PeerCreated(EnginePeerDescriptor),
    /// Notification that a peer has been destroyed.
    PeerDestroyed(PeerId),
    /// Notification that a peer has joined.
    PeerJoined(PeerId, PeerRoleId),
    /// Notification that a peer has left.
    PeerLeft(PeerId),
}

#[derive(Debug)]
pub enum MeetingUserEvent {
    /// Request to create a peer with the given ID and role.
    PeerCreate(PeerId, PeerRoleId),
    /// Request to destroy the peer with the given ID.
    PeerDestroy(PeerId),
    /// Response that a peer has been added.
    PeerAdded(Peer),
    /// Response that a peer has been removed.
    PeerRemoved(PeerId),
}

/// User-side container for peers and channels.
/// The goal of a meeting is to replicate a multi-peer environment.
pub struct Meeting {
    factory: Arc<PeerFactory>,
    engine_event: Duplex<MeetingEngineEvent>,
    user_event: Duplex<MeetingUserEvent>,
    peers: BTreeMap<PeerId, Vec<Channel>>,
    name: String,
}

impl Drop for Meeting {
    fn drop(&mut self) {
        let _ = self
            .engine_event
            .sender
            .send(MeetingEngineEvent::MeetingDestroyed);
        if cfg!(not(miri)) {
            tracing::event!(
                target: "tehuti::meeting",
                tracing::Level::TRACE,
                "Meeting {} closed",
                self.name
            );
        }
    }
}

impl Meeting {
    pub fn new(
        factory: Arc<PeerFactory>,
        engine_event: Duplex<MeetingEngineEvent>,
        user_event: Duplex<MeetingUserEvent>,
        name: impl ToString,
    ) -> Self {
        let name = name.to_string();
        if cfg!(not(miri)) {
            tracing::event!(
                target: "tehuti::meeting",
                tracing::Level::TRACE,
                "Meeting {} opened",
                name
            );
        }
        Self {
            factory,
            engine_event,
            user_event,
            peers: Default::default(),
            name,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn pump(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut result = false;
        if let Ok(event) = self.engine_event.receiver.try_recv() {
            self.handle_engine_event(event)?;
            result = true;
        }
        if let Ok(event) = self.user_event.receiver.try_recv() {
            self.handle_user_event(event)?;
            result = true;
        }
        for peer in self.peers.values_mut() {
            for channel in peer {
                result = result || channel.pump()?;
            }
        }
        Ok(result)
    }

    pub fn pump_all(&mut self) -> Result<bool, Box<dyn Error>> {
        let mut result = false;
        while let Ok(event) = self.engine_event.receiver.try_recv() {
            self.handle_engine_event(event)?;
            result = true;
        }
        while let Ok(event) = self.user_event.receiver.try_recv() {
            self.handle_user_event(event)?;
            result = true;
        }
        for peer in self.peers.values_mut() {
            for channel in peer {
                result = result || channel.pump_all()? > 0;
            }
        }
        Ok(result)
    }

    fn handle_engine_event(&mut self, event: MeetingEngineEvent) -> Result<(), Box<dyn Error>> {
        if cfg!(not(miri)) {
            tracing::event!(
                target: "tehuti::meeting",
                tracing::Level::TRACE,
                "Meeting {} handle engine event: {:?}",
                self.name,
                event
            );
        }
        match event {
            MeetingEngineEvent::PeerJoined(peer_id, role_id) => {
                let PeerBuildResult {
                    peer,
                    channels,
                    descriptor,
                } = self
                    .factory
                    .create(peer_id, role_id, true, self.user_event.sender.clone())?;
                if self.peers.contains_key(&peer.info().peer_id) {
                    return Err(format!("Peer {:?} already exists", peer.info().peer_id).into());
                }
                self.peers.insert(peer.info().peer_id, channels);
                self.engine_event
                    .sender
                    .send(MeetingEngineEvent::PeerCreated(descriptor))
                    .map_err(|err| format!("Engine event sender error: {err}"))?;
                self.user_event
                    .sender
                    .send(MeetingUserEvent::PeerAdded(peer))
                    .map_err(|err| format!("User event sender error: {err}"))?;
            }
            MeetingEngineEvent::PeerLeft(peer_id) => {
                if self.peers.remove(&peer_id).is_some() {
                    self.peers.remove(&peer_id);
                    self.engine_event
                        .sender
                        .send(MeetingEngineEvent::PeerDestroyed(peer_id))
                        .map_err(|err| format!("Engine event sender error: {err}"))?;
                }
                self.user_event
                    .sender
                    .send(MeetingUserEvent::PeerRemoved(peer_id))
                    .map_err(|err| format!("User event sender error: {err}"))?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_user_event(&mut self, event: MeetingUserEvent) -> Result<(), Box<dyn Error>> {
        if cfg!(not(miri)) {
            tracing::event!(
                target: "tehuti::meeting",
                tracing::Level::TRACE,
                "Meeting {} handle user event: {:?}",
                self.name,
                event
            );
        }
        match event {
            MeetingUserEvent::PeerCreate(peer_id, role_id) => {
                let PeerBuildResult {
                    peer,
                    channels,
                    descriptor,
                } = self
                    .factory
                    .create(peer_id, role_id, false, self.user_event.sender.clone())?;
                if self.peers.contains_key(&peer.info().peer_id) {
                    return Err(format!("Peer {:?} already exists", peer.info().peer_id).into());
                }
                self.peers.insert(peer.info().peer_id, channels);
                self.engine_event
                    .sender
                    .send(MeetingEngineEvent::PeerCreated(descriptor))
                    .map_err(|err| format!("Engine event sender error: {err}"))?;
                self.user_event
                    .sender
                    .send(MeetingUserEvent::PeerAdded(peer))
                    .map_err(|err| format!("User event sender error: {err}"))?;
            }
            MeetingUserEvent::PeerDestroy(peer_id) => {
                if self.peers.remove(&peer_id).is_some() {
                    self.peers.remove(&peer_id);
                    self.engine_event
                        .sender
                        .send(MeetingEngineEvent::PeerLeft(peer_id))
                        .map_err(|err| format!("Engine event sender error: {err}"))?;
                    self.user_event
                        .sender
                        .send(MeetingUserEvent::PeerRemoved(peer_id))
                        .map_err(|err| format!("User event sender error: {err}"))?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Future for Meeting {
    type Output = Result<(), Box<dyn Error>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.pump() {
            Ok(true) => Poll::Ready(Ok(())),
            Ok(false) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

pub struct MeetingInterfaceResult {
    pub meeting: Meeting,
    pub interface: MeetingInterface,
    pub engine_event: Duplex<MeetingEngineEvent>,
}

pub struct MeetingInterface {
    user_event: Duplex<MeetingUserEvent>,
}

impl MeetingInterface {
    pub fn make(factory: Arc<PeerFactory>, name: impl ToString) -> MeetingInterfaceResult {
        let (inside_engine, outside_engine) = Duplex::crossing_unbounded();
        let (inside_user, outside_user) = Duplex::crossing_unbounded();
        let meeting = Meeting::new(factory, inside_engine, inside_user, name);
        MeetingInterfaceResult {
            meeting,
            interface: Self {
                user_event: outside_user,
            },
            engine_event: outside_engine,
        }
    }
}

impl Deref for MeetingInterface {
    type Target = Duplex<MeetingUserEvent>;

    fn deref(&self) -> &Self::Target {
        &self.user_event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_async() {
        fn is_send<T: Send>() {}

        is_send::<MeetingEngineEvent>();
        is_send::<MeetingUserEvent>();
        is_send::<Meeting>();
    }
}
