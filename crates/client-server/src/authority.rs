use serde::{Deserialize, Serialize};
use std::{collections::HashMap, error::Error};
use tehuti::{
    channel::{ChannelId, ChannelMode, Dispatch},
    codec::postcard::PostcardCodec,
    event::{Duplex, Sender},
    meeting::{MeetingInterface, MeetingUserEvent},
    peer::{
        Peer, PeerBuilder, PeerDestructurer, PeerFactory, PeerId, PeerRoleId, TypedPeer,
        TypedPeerRole,
    },
    replica::ReplicaId,
    third_party::typid::ID,
};

const AUTHORITY_ROLE: PeerRoleId = PeerRoleId::new(0);
const AUTHORITY_PEER: PeerId = PeerId::new(0);
const AUTHORITY_EVENT_CHANNEL: ChannelId = ChannelId::new(0);

type EventId = ID<AuthorityEvent>;

#[derive(Debug, Serialize, Deserialize)]
enum AuthorityEvent {
    Request(EventId, AuthorityRequest),
    Response(EventId, AuthorityResponse),
}

#[derive(Debug, Serialize, Deserialize)]
enum AuthorityRequest {
    CreatePeer(PeerRoleId),
    DestroyPeer(PeerId),
    ReplicaId,
}

#[derive(Debug, Serialize, Deserialize)]
enum AuthorityResponse {
    CreatePeer(PeerId, PeerRoleId),
    ReplicaId(ReplicaId),
}

pub struct AuthorityUserData {
    pub is_server: bool,
}

pub type PureAuthority = Authority<()>;

pub struct Authority<Extension: TypedPeer> {
    meeting: MeetingInterface,
    role: Option<Role<Extension>>,
    added_peers: Sender<Peer>,
    removed_peers: Sender<PeerId>,
}

impl<Extension: TypedPeer> Drop for Authority<Extension> {
    fn drop(&mut self) {
        if self.role.take().is_some() {
            let _ = self
                .meeting
                .sender
                .send(MeetingUserEvent::PeerDestroy(AUTHORITY_PEER));
        }
    }
}

impl<Extension: TypedPeer + 'static> Authority<Extension> {
    pub fn peer_factory(is_server: bool) -> Result<PeerFactory, Box<dyn Error>> {
        Ok(PeerFactory::default()
            .with_user_data(AuthorityUserData { is_server })?
            .with_typed::<Role<Extension>>())
    }
}

impl<Extension: TypedPeer> Authority<Extension> {
    pub fn new(
        is_server: bool,
        meeting: MeetingInterface,
        added_peers: Sender<Peer>,
        removed_peers: Sender<PeerId>,
    ) -> Result<Self, Box<dyn Error>> {
        if is_server {
            meeting
                .sender
                .send(MeetingUserEvent::PeerCreate(AUTHORITY_PEER, AUTHORITY_ROLE))?;
        }
        Ok(Self {
            meeting,
            role: None,
            added_peers,
            removed_peers,
        })
    }

    pub fn is_initialized(&self) -> bool {
        self.role.is_some()
    }

    pub fn is_server(&self) -> bool {
        matches!(self.role, Some(Role::Server { .. }))
    }

    pub fn is_client(&self) -> bool {
        matches!(self.role, Some(Role::Client { .. }))
    }

    pub fn extension(&self) -> Option<&Extension> {
        match &self.role {
            Some(Role::Server { extension, .. }) => Some(extension),
            Some(Role::Client { extension, .. }) => Some(extension),
            None => None,
        }
    }

    pub fn extension_mut(&mut self) -> Option<&mut Extension> {
        match &mut self.role {
            Some(Role::Server { extension, .. }) => Some(extension),
            Some(Role::Client { extension, .. }) => Some(extension),
            None => None,
        }
    }

    pub fn create_peer(&mut self, role_id: PeerRoleId) -> Result<(), Box<dyn Error>> {
        if role_id == AUTHORITY_ROLE {
            return Err("Cannot create Authority peer".into());
        }
        match self.role.as_mut() {
            Some(Role::Server {
                peer_id_generator, ..
            }) => {
                *peer_id_generator += 1;
                let peer_id = PeerId::new(*peer_id_generator);
                self.meeting
                    .sender
                    .send(MeetingUserEvent::PeerCreate(peer_id, role_id))?;
            }
            Some(Role::Client { events, .. }) => {
                let event_id = EventId::new();
                events.sender.send(
                    AuthorityEvent::Request(event_id, AuthorityRequest::CreatePeer(role_id)).into(),
                )?;
            }
            _ => return Err("Authority role not initialized as Client".into()),
        }
        Ok(())
    }

    pub fn destroy_peer(&mut self, peer_id: PeerId) -> Result<(), Box<dyn Error>> {
        if peer_id == AUTHORITY_PEER {
            return Err("Cannot destroy Authority peer".into());
        }
        match self.role.as_mut() {
            Some(Role::Server { .. }) => {
                self.meeting
                    .sender
                    .send(MeetingUserEvent::PeerDestroy(peer_id))?;
            }
            Some(Role::Client { events, .. }) => {
                let event_id = EventId::new();
                events.sender.send(
                    AuthorityEvent::Request(event_id, AuthorityRequest::DestroyPeer(peer_id))
                        .into(),
                )?;
            }
            _ => return Err("Authority role not initialized as Client".into()),
        }
        Ok(())
    }

    pub fn request_replica_id(&mut self, reply: Sender<ReplicaId>) -> Result<(), Box<dyn Error>> {
        match self.role.as_mut() {
            Some(Role::Server {
                replica_id_generator,
                ..
            }) => {
                *replica_id_generator += 1;
                let replica_id = ReplicaId::new(*replica_id_generator);
                reply.send(replica_id)?;
            }
            Some(Role::Client {
                events,
                replica_id_requests,
                ..
            }) => {
                let event_id = EventId::new();
                replica_id_requests.insert(event_id, reply);
                events
                    .sender
                    .send(AuthorityEvent::Request(event_id, AuthorityRequest::ReplicaId).into())?;
            }
            _ => return Err("Authority role not initialized as Client".into()),
        }
        Ok(())
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        while let Some(event) = self.meeting.receiver.try_recv() {
            match event {
                MeetingUserEvent::PeerAdded(peer) => {
                    if peer.info().peer_id == AUTHORITY_PEER {
                        let role = peer.into_typed::<Role<Extension>>()?;
                        self.role = Some(role);
                    } else {
                        self.added_peers.send(peer)?;
                    }
                }
                MeetingUserEvent::PeerRemoved(peer_id) => {
                    if peer_id == AUTHORITY_PEER {
                        self.role = None;
                    } else {
                        self.removed_peers.send(peer_id)?;
                    }
                }
                _ => {}
            }
        }

        match &mut self.role {
            Some(Role::Server {
                events,
                peer_id_generator,
                replica_id_generator,
                ..
            }) => {
                for Dispatch {
                    message, sender, ..
                } in events.receiver.iter()
                {
                    if let AuthorityEvent::Request(event_id, request) = message {
                        match request {
                            AuthorityRequest::CreatePeer(role_id) => {
                                *peer_id_generator += 1;
                                let peer_id = PeerId::new(*peer_id_generator);
                                events.sender.send(
                                    Dispatch::new(AuthorityEvent::Response(
                                        event_id,
                                        AuthorityResponse::CreatePeer(peer_id, role_id),
                                    ))
                                    .maybe_recepient(sender),
                                )?;
                            }
                            AuthorityRequest::DestroyPeer(peer_id) => {
                                if peer_id == AUTHORITY_PEER {
                                    return Err("Cannot destroy Authority peer".into());
                                }
                                self.meeting
                                    .sender
                                    .send(MeetingUserEvent::PeerDestroy(peer_id))?;
                            }
                            AuthorityRequest::ReplicaId => {
                                *replica_id_generator += 1;
                                let replica_id = ReplicaId::new(*replica_id_generator);
                                events.sender.send(
                                    Dispatch::new(AuthorityEvent::Response(
                                        event_id,
                                        AuthorityResponse::ReplicaId(replica_id),
                                    ))
                                    .maybe_recepient(sender),
                                )?;
                            }
                        }
                    }
                }
            }
            Some(Role::Client {
                events,
                replica_id_requests,
                ..
            }) => {
                for _ in replica_id_requests.extract_if(|_, receiver| receiver.is_disconnected()) {}
                for Dispatch { message, .. } in events.receiver.iter() {
                    if let AuthorityEvent::Response(event_id, response) = message {
                        match response {
                            AuthorityResponse::CreatePeer(peer_id, role_id) => {
                                if role_id == AUTHORITY_ROLE {
                                    return Err("Received Authority peer from Authority".into());
                                }
                                self.meeting
                                    .sender
                                    .send(MeetingUserEvent::PeerCreate(peer_id, role_id))?;
                            }
                            AuthorityResponse::ReplicaId(replica_id) => {
                                if let Some(reply) = replica_id_requests.remove(&event_id) {
                                    reply.send(replica_id)?;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}

enum Role<Extension: TypedPeer> {
    Server {
        events: Duplex<Dispatch<AuthorityEvent>>,
        peer_id_generator: u64,
        replica_id_generator: u64,
        extension: Extension,
    },
    Client {
        events: Duplex<Dispatch<AuthorityEvent>>,
        replica_id_requests: HashMap<EventId, Sender<ReplicaId>>,
        extension: Extension,
    },
}

impl<Extension: TypedPeer> TypedPeerRole for Role<Extension> {
    const ROLE_ID: PeerRoleId = AUTHORITY_ROLE;
}

impl<Extension: TypedPeer> TypedPeer for Role<Extension> {
    fn builder(builder: PeerBuilder) -> Result<PeerBuilder, Box<dyn Error>> {
        if builder.info().peer_id != AUTHORITY_PEER {
            return Ok(builder);
        }

        let builder = builder.bind_read_write::<PostcardCodec<AuthorityEvent>, AuthorityEvent>(
            AUTHORITY_EVENT_CHANNEL,
            ChannelMode::ReliableOrdered,
            None,
        );
        Extension::builder(builder)
    }

    fn into_typed(mut peer: PeerDestructurer) -> Result<Self, Box<dyn Error>> {
        if peer.info().peer_id != AUTHORITY_PEER {
            return Err("Invalid Authority peer ID".into());
        }
        let is_server = peer.user_data().access::<AuthorityUserData>()?.is_server;
        let events = peer.read_write::<AuthorityEvent>(AUTHORITY_EVENT_CHANNEL)?;

        if is_server {
            Ok(Self::Server {
                events,
                peer_id_generator: 0,
                replica_id_generator: 0,
                extension: Extension::into_typed(peer)?,
            })
        } else {
            Ok(Self::Client {
                events,
                replica_id_requests: Default::default(),
                extension: Extension::into_typed(peer)?,
            })
        }
    }
}
