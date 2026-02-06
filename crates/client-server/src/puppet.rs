use std::{
    error::Error,
    ops::{Deref, DerefMut},
};
use tehuti::replica::{Replica, ReplicaApplyChanges, ReplicaCollectChanges};

pub trait Puppetable {
    #[allow(unused_variables)]
    fn collect_changes(&mut self, collector: ReplicaCollectChanges) -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    #[allow(unused_variables)]
    fn apply_changes(&mut self, applicator: ReplicaApplyChanges) -> Result<(), Box<dyn Error>> {
        Ok(())
    }
}

pub struct Puppet<T: Puppetable> {
    replica: Replica,
    data: T,
}

impl<T: Puppetable> Puppet<T> {
    pub fn new(replica: Replica, data: T) -> Self {
        Self { replica, data }
    }

    pub fn replica(&self) -> &Replica {
        &self.replica
    }

    pub fn replicate(&mut self) -> Result<(), Box<dyn Error>> {
        while let Some(applicator) = self.replica.apply_changes() {
            self.data.apply_changes(applicator)?;
        }
        if let Some(collector) = self.replica.collect_changes() {
            self.data.collect_changes(collector)?;
        }
        Ok(())
    }
}

impl<T: Puppetable> Deref for Puppet<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: Puppetable> DerefMut for Puppet<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}
