use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ops::{Add, AddAssign, Sub, SubAssign},
};
use tehuti::replication::{BufferRead, BufferWrite, Replicable};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TimeStamp(u64);

impl TimeStamp {
    pub const fn new(ticks: u64) -> Self {
        Self(ticks)
    }

    pub const fn ticks(&self) -> u64 {
        self.0
    }
}

impl Add<u64> for TimeStamp {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0.saturating_add(rhs))
    }
}

impl AddAssign<u64> for TimeStamp {
    fn add_assign(&mut self, rhs: u64) {
        self.0 = self.0.saturating_add(rhs);
    }
}

impl Sub<u64> for TimeStamp {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0.saturating_sub(rhs))
    }
}

impl SubAssign<u64> for TimeStamp {
    fn sub_assign(&mut self, rhs: u64) {
        self.0 = self.0.saturating_sub(rhs);
    }
}

impl Sub for TimeStamp {
    type Output = usize;

    fn sub(self, rhs: Self) -> Self::Output {
        self.0.saturating_sub(rhs.0) as usize
    }
}

impl Replicable for TimeStamp {
    fn collect_changes(&self, buffer: &mut BufferWrite) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut BufferRead) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for TimeStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#timestamp:{}", self.0)
    }
}
