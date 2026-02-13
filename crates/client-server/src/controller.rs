use crate::authority::Authority;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ops::{Deref, DerefMut},
};
use tehuti::{
    channel::{ChannelId, Dispatch},
    event::{Receiver, Sender, unbounded},
    peer::{Peer, TypedPeer},
    replica::{Replica, ReplicaId, ReplicaSet},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControllerEvent {
    CreateReplica(ReplicaId),
    DestroyReplica(ReplicaId),
    SynchronizeReplicaSetRequest,
    SynchronizeReplicaSetResponse(Vec<ReplicaId>),
}

pub struct Controller {
    peer: Peer,
    replica_set: ReplicaSet,
    event_sender: Sender<Dispatch<ControllerEvent>>,
    event_receiver: Receiver<Dispatch<ControllerEvent>>,
    replica_id_replies: Vec<Receiver<ReplicaId>>,
    replica_added_sender: Sender<Replica>,
    replica_removed_sender: Sender<ReplicaId>,
}

impl Controller {
    pub fn new(
        peer: Peer,
        controller_channel_id: ChannelId,
        change_channel_id: Option<ChannelId>,
        rpc_channel_id: Option<ChannelId>,
        replica_added_sender: Sender<Replica>,
        replica_removed_sender: Sender<ReplicaId>,
    ) -> Result<Self, Box<dyn Error>> {
        let mut replica_set = ReplicaSet::default();
        replica_set.bind_peer(&peer, change_channel_id, rpc_channel_id);
        let event_sender = peer
            .sender::<ControllerEvent>(controller_channel_id)?
            .clone();
        let event_receiver = peer
            .receiver::<ControllerEvent>(controller_channel_id)?
            .clone();
        if peer.info().remote {
            event_sender.send(ControllerEvent::SynchronizeReplicaSetRequest.into())?;
        }
        Ok(Self {
            peer,
            replica_set,
            event_sender,
            event_receiver,
            replica_id_replies: Default::default(),
            replica_added_sender,
            replica_removed_sender,
        })
    }

    pub fn create_replica<Extension: TypedPeer>(
        &mut self,
        authority: &mut Authority<Extension>,
    ) -> Result<(), Box<dyn Error>> {
        if self.peer.info().remote {
            return Ok(());
        }
        let (replica_id_sender, replica_id_receiver) = unbounded();
        authority.request_replica_id(replica_id_sender)?;
        self.replica_id_replies.push(replica_id_receiver);
        Ok(())
    }

    pub fn destroy_replica(&mut self, replica_id: ReplicaId) -> Result<(), Box<dyn Error>> {
        if self.peer.info().remote {
            return Ok(());
        }
        if !self.replica_set.destroy(&replica_id) {
            return Err(format!("Replica {:?} does not exist", replica_id).into());
        }
        self.event_sender
            .send(ControllerEvent::DestroyReplica(replica_id).into())?;
        Ok(())
    }

    pub fn replicas(&self) -> impl Iterator<Item = ReplicaId> {
        self.replica_set.iter()
    }

    pub fn maintain(&mut self) -> Result<(), Box<dyn Error>> {
        let mut replica_id_replies = Vec::with_capacity(self.replica_id_replies.len());
        for replica_id_receiver in self.replica_id_replies.drain(..) {
            if let Some(replica_id) = replica_id_receiver.try_recv() {
                let replica = self.replica_set.create(replica_id)?;
                self.replica_added_sender.send(replica)?;
                self.event_sender
                    .send(ControllerEvent::CreateReplica(replica_id).into())?;
            } else if replica_id_receiver.is_connected() {
                replica_id_replies.push(replica_id_receiver);
            }
        }
        self.replica_id_replies = replica_id_replies;

        for event in self.event_receiver.iter() {
            match event.message {
                ControllerEvent::CreateReplica(replica_id) => {
                    let replica = self.replica_set.create(replica_id)?;
                    self.replica_added_sender.send(replica)?;
                }
                ControllerEvent::DestroyReplica(replica_id) => {
                    self.replica_removed_sender.send(replica_id)?;
                }
                ControllerEvent::SynchronizeReplicaSetRequest => {
                    if !self.peer.info().remote {
                        self.event_sender.send(
                            ControllerEvent::SynchronizeReplicaSetResponse(
                                self.replica_set.iter().collect(),
                            )
                            .into(),
                        )?;
                    }
                }
                ControllerEvent::SynchronizeReplicaSetResponse(replica_ids) => {
                    if self.peer.info().remote {
                        let to_remove = self
                            .replica_set
                            .iter()
                            .filter(|replica_id| !replica_ids.contains(replica_id))
                            .collect::<Vec<_>>();
                        for replica_id in to_remove {
                            self.replica_set.destroy(&replica_id);
                            self.replica_removed_sender.send(replica_id)?;
                        }
                        for replica_id in replica_ids {
                            if !self.replica_set.has(&replica_id) {
                                let replica = self.replica_set.create(replica_id)?;
                                self.replica_added_sender.send(replica)?;
                            }
                        }
                    }
                }
            }
        }

        self.replica_set.maintain();

        Ok(())
    }
}

impl Deref for Controller {
    type Target = Peer;

    fn deref(&self) -> &Self::Target {
        &self.peer
    }
}

impl DerefMut for Controller {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.peer
    }
}
