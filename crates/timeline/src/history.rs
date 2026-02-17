use crate::time::TimeStamp;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    error::Error,
    ops::{Bound, RangeBounds, RangeInclusive},
};
use tehuti::{buffer::Buffer, replication::Replicable};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct HistoryBuffer<T: Clone> {
    capacity: usize,
    buffer: VecDeque<T>,
    now: TimeStamp,
}

impl<T: Clone> HistoryBuffer<T> {
    pub fn new(now: TimeStamp, items: impl IntoIterator<Item = T>) -> Self {
        let buffer = items.into_iter().collect::<VecDeque<_>>();
        Self {
            capacity: buffer.len(),
            buffer,
            now,
        }
    }

    pub fn snapshot(now: TimeStamp, value: T) -> Self {
        Self {
            capacity: 1,
            buffer: VecDeque::from([value]),
            now,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity,
            buffer: VecDeque::with_capacity(capacity),
            now: Default::default(),
        }
    }

    pub fn clone_to(&self, other: &mut Self, range: impl RangeBounds<TimeStamp> + Clone) {
        for (timestamp, value) in self.iter(range) {
            other.set(timestamp, value.clone());
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.len() > capacity {
            self.buffer.pop_front();
        }
    }

    pub fn now(&self) -> Option<TimeStamp> {
        if self.is_empty() {
            None
        } else {
            Some(self.now)
        }
    }

    pub fn past(&self) -> Option<TimeStamp> {
        if self.is_empty() {
            None
        } else {
            Some(TimeStamp::new(
                self.now
                    .ticks()
                    .saturating_sub(self.buffer.len().saturating_sub(1) as u64),
            ))
        }
    }

    pub fn range(&self) -> Option<RangeInclusive<TimeStamp>> {
        Some(self.past()?..=self.now()?)
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn iter(
        &self,
        range: impl RangeBounds<TimeStamp> + Clone,
    ) -> impl DoubleEndedIterator<Item = (TimeStamp, &T)> {
        self.past().into_iter().flat_map(move |past| {
            let range = range.clone();
            self.buffer
                .iter()
                .enumerate()
                .map(move |(index, value)| (past + index as u64, value))
                .filter(move |(timestamp, _)| range.contains(timestamp))
        })
    }

    pub fn set(&mut self, timestamp: TimeStamp, value: T) {
        let range = self.range();
        if let Some(range) = range {
            if timestamp < *range.start() {
                let diff = *range.start() - timestamp;
                if diff >= self.capacity {
                    self.buffer = std::iter::repeat_n(value, self.capacity).collect();
                } else {
                    for _ in 0..diff {
                        self.buffer.push_front(value.clone());
                    }
                    while self.len() > self.capacity {
                        self.buffer.pop_back();
                    }
                }
                self.now = self
                    .now
                    .min(timestamp + self.len().saturating_sub(1) as u64);
            } else if timestamp > *range.end() {
                let duplicate_value = self.buffer.back().cloned().unwrap();
                let diff = timestamp - *range.end();
                if diff >= self.capacity {
                    self.buffer = std::iter::repeat_n(duplicate_value, self.capacity - 1)
                        .chain(std::iter::once(value))
                        .collect();
                } else {
                    for _ in 0..diff - 1 {
                        self.buffer.push_back(duplicate_value.clone());
                    }
                    self.buffer.push_back(value);
                    while self.len() > self.capacity {
                        self.buffer.pop_front();
                    }
                }
                self.now = timestamp;
            } else {
                let index = self.timestamp_to_index(timestamp).unwrap();
                self.buffer[index] = value;
            }
        } else {
            self.buffer.push_back(value);
            self.now = timestamp;
        }
    }

    pub fn time_travel_to(&mut self, timestamp: TimeStamp) -> bool {
        let result = if let Some(range) = self.range() {
            if timestamp < *range.start() {
                self.buffer.clear();
            } else if timestamp < *range.end() {
                let len = timestamp - *range.start() + 1;
                self.buffer.truncate(len);
            } else if timestamp > *range.end() {
                let duplicate_value = self.buffer.back().cloned().unwrap();
                if timestamp - *range.end() >= self.capacity {
                    self.buffer = std::iter::repeat_n(duplicate_value, self.capacity).collect();
                } else {
                    for _ in 0..(timestamp - *range.end()) {
                        self.buffer.push_back(duplicate_value.clone());
                        while self.len() > self.capacity {
                            self.buffer.pop_front();
                        }
                    }
                }
            }
            true
        } else {
            false
        };
        self.now = timestamp;
        result
    }

    pub fn ensure_timestamp(&mut self, timestamp: TimeStamp, default: impl Fn() -> T) {
        let Some(range) = self.range() else {
            self.set(timestamp, default());
            return;
        };
        if timestamp < *range.start() {
            self.set(timestamp, default());
        } else if timestamp > *range.end() {
            self.set(timestamp, self.buffer.back().cloned().unwrap());
        }
    }

    pub fn get(&self, timestamp: TimeStamp) -> Option<&T> {
        let index = self.timestamp_to_index(timestamp)?;
        self.buffer.get(index)
    }

    pub fn get_mut(&mut self, timestamp: TimeStamp) -> Option<&mut T> {
        let index = self.timestamp_to_index(timestamp)?;
        self.buffer.get_mut(index)
    }

    pub fn get_extrapolated(&self, timestamp: TimeStamp) -> Option<&T> {
        let range = self.range()?;
        if range.contains(&timestamp) {
            self.get(timestamp)
        } else if timestamp < *range.start() {
            self.buffer.front()
        } else {
            self.buffer.back()
        }
    }

    pub fn clear(&mut self)
    where
        T: Default,
    {
        self.buffer.clear();
        self.now = Default::default();
    }

    pub fn has(&self, timestamp: TimeStamp) -> bool {
        self.get(timestamp).is_some()
    }

    pub fn divergence(&self, other: &Self) -> Option<TimeStamp>
    where
        T: PartialEq,
    {
        let self_range = self.range()?;
        let other_range = other.range()?;
        let start_timestamp = self_range.start().max(other_range.start());
        let end_timestamp = self_range.end().min(other_range.end());
        (start_timestamp.ticks()..=end_timestamp.ticks())
            .map(TimeStamp::new)
            .find(|timestamp| self.get(*timestamp) != other.get(*timestamp))
    }

    pub fn collect_changes(
        &self,
        buffer: &mut Buffer,
        range: impl RangeBounds<TimeStamp> + Clone,
        compress_delta: bool,
    ) -> Result<(), Box<dyn Error>>
    where
        T: PartialEq + Replicable,
    {
        let range_start = match range.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => *start + 1,
            Bound::Unbounded => self.past().unwrap_or_default(),
        }
        .max(self.past().unwrap_or_default());
        let range_end = match range.end_bound() {
            Bound::Included(end) => *end,
            Bound::Excluded(end) => *end - 1,
            Bound::Unbounded => self.now().unwrap_or_default(),
        }
        .min(self.now);
        if compress_delta {
            true.collect_changes(buffer)?;
            range_start.collect_changes(buffer)?;
            range_end.collect_changes(buffer)?;
            let mut prev_value = None;
            for tick in range_start.ticks()..=range_end.ticks() {
                let timestamp = TimeStamp::new(tick);
                let Some(value) = self.get(timestamp) else {
                    return Err(format!("Missing value for timestamp {}", timestamp.ticks()).into());
                };
                if prev_value.is_none() || prev_value.as_ref().unwrap() != value {
                    true.collect_changes(buffer)?;
                    value.collect_changes(buffer)?;
                    prev_value = Some(value.clone());
                } else {
                    false.collect_changes(buffer)?;
                }
            }
        } else {
            false.collect_changes(buffer)?;
            range_start.collect_changes(buffer)?;
            range_end.collect_changes(buffer)?;
            for tick in range_start.ticks()..=range_end.ticks() {
                let timestamp = TimeStamp::new(tick);
                let Some(value) = self.get(timestamp) else {
                    return Err(format!("Missing value for timestamp {}", timestamp.ticks()).into());
                };
                value.collect_changes(buffer)?;
            }
        }
        Ok(())
    }

    pub fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>>
    where
        T: Default + Replicable,
    {
        let mut compressed = false;
        compressed.apply_changes(buffer)?;
        let mut from = TimeStamp::default();
        from.apply_changes(buffer)?;
        let mut to = TimeStamp::default();
        to.apply_changes(buffer)?;
        if compressed {
            let mut value = T::default();
            for tick in from.ticks()..=to.ticks() {
                let mut changed = false;
                changed.apply_changes(buffer)?;
                if changed {
                    value.apply_changes(buffer)?;
                }
                let timestamp = TimeStamp::new(tick);
                self.set(timestamp, value.clone());
            }
        } else {
            for tick in from.ticks()..=to.ticks() {
                let mut value = T::default();
                value.apply_changes(buffer)?;
                let timestamp = TimeStamp::new(tick);
                self.set(timestamp, value);
            }
        }
        Ok(())
    }

    fn timestamp_to_index(&self, timestamp: TimeStamp) -> Option<usize> {
        let past = self.past()?;
        let diff = timestamp.ticks().checked_sub(past.ticks())?;
        Some(diff as usize)
    }
}

impl<T: Clone + std::fmt::Debug> std::fmt::Display for HistoryBuffer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (timestamp, value) in self.iter(..) {
            writeln!(f, "{}: {:?}", timestamp, value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEvent<T> {
    start: TimeStamp,
    snapshots: SmallVec<[T; 4]>,
}

impl<T> HistoryEvent<T> {
    pub fn now(&self) -> TimeStamp {
        self.start + self.snapshots.len() as u64 - 1
    }

    pub fn past(&self) -> TimeStamp {
        self.start
    }

    pub fn range(&self) -> RangeInclusive<TimeStamp> {
        self.past()..=self.now()
    }

    pub fn iter(&self) -> impl Iterator<Item = (TimeStamp, &T)> {
        self.snapshots
            .iter()
            .enumerate()
            .map(move |(index, snapshot)| (self.start + index as u64, snapshot))
    }

    pub fn collect_history(
        buffer: &HistoryBuffer<T>,
        range: impl RangeBounds<TimeStamp> + Clone,
    ) -> Option<Self>
    where
        T: Clone,
    {
        let mut start = None;
        let snapshots = buffer
            .iter(range)
            .map(|(timestamp, snapshot)| {
                if start.is_none() {
                    start = Some(timestamp);
                }
                snapshot.clone()
            })
            .collect();
        Some(Self {
            start: start?,
            snapshots,
        })
    }

    pub fn collect_snapshot(buffer: &HistoryBuffer<T>, timestamp: TimeStamp) -> Option<Self>
    where
        T: Clone,
    {
        let snapshot = buffer.get(timestamp)?.clone();
        Some(Self {
            start: timestamp,
            snapshots: [snapshot].into_iter().collect(),
        })
    }

    pub fn apply_history(&self, buffer: &mut HistoryBuffer<T>) -> Result<(), Box<dyn Error>>
    where
        T: Clone,
    {
        let mut timestamp = self.start;
        for snapshot in &self.snapshots {
            buffer.set(timestamp, snapshot.clone());
            timestamp += 1;
        }
        Ok(())
    }

    pub fn apply_history_divergence(
        &self,
        buffer: &mut HistoryBuffer<T>,
    ) -> Result<Option<TimeStamp>, Box<dyn Error>>
    where
        T: Clone + PartialEq,
    {
        // TODO: optimize!
        let mut temp = HistoryBuffer::<T>::with_capacity(self.snapshots.capacity());
        self.apply_history(&mut temp)?;
        let divergence = buffer.divergence(&temp);
        temp.clone_to(buffer, ..);
        Ok(divergence)
    }
}

impl<T: std::fmt::Debug> std::fmt::Display for HistoryEvent<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (i, snapshot) in self.snapshots.iter().enumerate() {
            writeln!(f, "{}: {:?}", self.start + i as u64, snapshot)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    struct Input {
        run: bool,
        jump: bool,
    }

    impl Replicable for Input {
        fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
            self.run.collect_changes(buffer)?;
            self.jump.collect_changes(buffer)?;
            Ok(())
        }

        fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
            self.run.apply_changes(buffer)?;
            self.jump.apply_changes(buffer)?;
            Ok(())
        }
    }

    #[test]
    fn test_history_buffer_set() {
        let mut buffer = HistoryBuffer::with_capacity(3);
        assert_eq!(buffer.len(), 0);
        assert_eq!(buffer.now(), None);
        assert_eq!(buffer.past(), None);
        assert_eq!(buffer.iter(..).collect::<Vec<_>>(), vec![]);

        buffer.set(TimeStamp::new(0), 0);
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.now(), Some(TimeStamp::new(0)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![(TimeStamp::new(0), &0)]
        );

        buffer.set(TimeStamp::new(1), 1);
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.now(), Some(TimeStamp::new(1)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![(TimeStamp::new(0), &0), (TimeStamp::new(1), &1)]
        );

        buffer.set(TimeStamp::new(2), 2);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(2)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(0), &0),
                (TimeStamp::new(1), &1),
                (TimeStamp::new(2), &2)
            ]
        );

        buffer.set(TimeStamp::new(3), 3);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(3)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(1)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(1), &1),
                (TimeStamp::new(2), &2),
                (TimeStamp::new(3), &3)
            ]
        );

        buffer.set(TimeStamp::new(6), 6);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(6)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(4)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(4), &3),
                (TimeStamp::new(5), &3),
                (TimeStamp::new(6), &6)
            ]
        );

        buffer.set(TimeStamp::new(5), 5);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(6)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(4)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(4), &3),
                (TimeStamp::new(5), &5),
                (TimeStamp::new(6), &6)
            ]
        );

        buffer.set(TimeStamp::new(4), 4);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(6)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(4)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(4), &4),
                (TimeStamp::new(5), &5),
                (TimeStamp::new(6), &6)
            ]
        );

        buffer.set(TimeStamp::new(0), 0);
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.now(), Some(TimeStamp::new(2)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(0), &0),
                (TimeStamp::new(1), &0),
                (TimeStamp::new(2), &0)
            ]
        );

        buffer.set(TimeStamp::new(2), 2);
        buffer.set(TimeStamp::new(5), 5);
        buffer.time_travel_to(TimeStamp::new(4) - 1);
        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.now(), Some(TimeStamp::new(3)));
        assert_eq!(buffer.past(), Some(TimeStamp::new(3)));
        assert_eq!(
            buffer.iter(..).collect::<Vec<_>>(),
            vec![(TimeStamp::new(3), &2)]
        );
    }

    #[test]
    fn test_history_buffer_clone_to() {
        let mut a = HistoryBuffer::with_capacity(4);
        a.set(TimeStamp::new(0), 0);
        a.set(TimeStamp::new(1), 1);
        a.set(TimeStamp::new(2), 2);

        let mut b = HistoryBuffer::with_capacity(4);
        a.clone_to(&mut b, ..);
        assert_eq!(a, b);

        let mut c = HistoryBuffer::with_capacity(4);
        a.clone_to(&mut c, TimeStamp::new(1)..);
        assert_eq!(
            c.iter(..).collect::<Vec<_>>(),
            vec![(TimeStamp::new(1), &1), (TimeStamp::new(2), &2)]
        );

        let mut d = HistoryBuffer::with_capacity(4);
        a.clone_to(&mut d, ..=TimeStamp::new(1));
        assert_eq!(
            d.iter(..).collect::<Vec<_>>(),
            vec![(TimeStamp::new(0), &0), (TimeStamp::new(1), &1)]
        );

        let mut d = HistoryBuffer::with_capacity(4);
        d.set(TimeStamp::new(1), 42);
        d.clone_to(&mut a, TimeStamp::new(2)..);
        assert_eq!(
            a.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(0), &0),
                (TimeStamp::new(1), &1),
                (TimeStamp::new(2), &2)
            ]
        );
        d.clone_to(&mut a, TimeStamp::new(1)..=TimeStamp::new(1));
        assert_eq!(
            a.iter(..).collect::<Vec<_>>(),
            vec![
                (TimeStamp::new(0), &0),
                (TimeStamp::new(1), &42),
                (TimeStamp::new(2), &2)
            ]
        );
    }

    #[test]
    fn test_history_buffer_divergence() {
        let mut a = HistoryBuffer::with_capacity(4);
        a.set(
            TimeStamp::new(0),
            Input {
                run: true,
                jump: false,
            },
        );
        a.set(
            TimeStamp::new(1),
            Input {
                run: false,
                jump: false,
            },
        );
        a.set(
            TimeStamp::new(2),
            Input {
                run: false,
                jump: true,
            },
        );
        a.set(
            TimeStamp::new(3),
            Input {
                run: false,
                jump: true,
            },
        );

        let mut b = HistoryBuffer::with_capacity(4);
        b.set(
            TimeStamp::new(0),
            Input {
                run: true,
                jump: false,
            },
        );
        b.set(
            TimeStamp::new(1),
            Input {
                run: false,
                jump: false,
            },
        );
        b.set(
            TimeStamp::new(2),
            Input {
                run: false,
                jump: false,
            },
        );
        b.set(
            TimeStamp::new(3),
            Input {
                run: false,
                jump: true,
            },
        );

        let divergence = a.divergence(&b).unwrap();
        assert_eq!(divergence, TimeStamp::new(2));

        a.time_travel_to(divergence - 1);
        assert_eq!(a.len(), 2);
        assert_eq!(a.now(), Some(TimeStamp::new(1)));
        assert_eq!(a.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            a.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &Input {
                        run: true,
                        jump: false
                    }
                ),
                (
                    TimeStamp::new(1),
                    &Input {
                        run: false,
                        jump: false
                    }
                )
            ]
        );
    }

    #[test]
    fn test_history_buffer_replication() {
        let mut a = HistoryBuffer::with_capacity(4);
        a.set(
            TimeStamp::new(0),
            Input {
                run: true,
                jump: true,
            },
        );
        a.set(
            TimeStamp::new(1),
            Input {
                run: true,
                jump: true,
            },
        );
        a.set(
            TimeStamp::new(2),
            Input {
                run: false,
                jump: false,
            },
        );

        let mut buffer = Buffer::new(Default::default());
        a.collect_changes(&mut buffer, .., true).unwrap();
        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 10);

        let mut b = HistoryBuffer::with_capacity(4);
        let mut buffer = Buffer::new(buffer);
        b.apply_changes(&mut buffer).unwrap();

        assert_eq!(a, b);

        let mut buffer = Buffer::new(Default::default());
        a.collect_changes(&mut buffer, .., false).unwrap();
        let buffer = buffer.into_inner();
        assert_eq!(buffer.len(), 9);

        let mut b = HistoryBuffer::with_capacity(4);
        let mut buffer = Buffer::new(buffer);
        b.apply_changes(&mut buffer).unwrap();

        assert_eq!(a, b);
    }

    #[test]
    fn test_history_event() {
        let mut buffer = HistoryBuffer::with_capacity(4);
        buffer.set(TimeStamp::new(0), 0);
        buffer.set(TimeStamp::new(1), 1);
        buffer.set(TimeStamp::new(2), 2);

        let event = HistoryEvent::collect_history(&buffer, ..).unwrap();
        assert_eq!(event.start, TimeStamp::new(0));
        assert_eq!(event.snapshots, SmallVec::from_buf([0, 1, 2]));

        let mut new_buffer = HistoryBuffer::with_capacity(4);
        event.apply_history(&mut new_buffer).unwrap();
        assert_eq!(buffer, new_buffer);

        let mut temp = HistoryBuffer::with_capacity(4);
        temp.set(TimeStamp::new(1), 10);
        temp.set(TimeStamp::new(2), 20);
        let divergence = event.apply_history_divergence(&mut temp).unwrap().unwrap();
        assert_eq!(divergence, TimeStamp::new(1));

        // TODO: should this scenario really give no divergence? rethink that.
        let mut temp = HistoryBuffer::with_capacity(4);
        temp.set(TimeStamp::new(10), 10);
        assert!(event.apply_history_divergence(&mut temp).unwrap().is_none());
    }
}
