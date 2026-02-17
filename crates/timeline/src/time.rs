use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    ops::{Add, AddAssign, Sub, SubAssign},
};
use tehuti::{buffer::Buffer, codec::Codec, replication::Replicable};

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

    pub fn possibly_oldest(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    pub fn find_possibly_oldest(timestamps: impl Iterator<Item = Option<Self>>) -> Option<Self> {
        timestamps.flatten().min()
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
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl Codec for TimeStamp {
    type Value = Self;

    fn encode(message: &Self::Value, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        u64::encode(&message.0, buffer)?;
        Ok(())
    }

    fn decode(buffer: &mut Buffer) -> Result<Self::Value, Box<dyn Error>> {
        let ticks = u64::decode(buffer)?;
        Ok(Self(ticks))
    }
}

impl std::fmt::Display for TimeStamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#timestamp:{}", self.0)
    }
}
