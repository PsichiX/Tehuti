use crate::{history::HistoryBuffer, time::TimeStamp};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use smallvec::SmallVec;
use std::{
    error::Error,
    ops::{Bound, RangeBounds, RangeInclusive},
};
use tehuti::{buffer::Buffer, replication::Replicable};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct CommandId(u64);

impl CommandId {
    pub const fn new(ticks: u64) -> Self {
        Self(ticks)
    }

    pub const fn id(&self) -> u64 {
        self.0
    }
}

impl Replicable for CommandId {
    fn collect_changes(&self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.collect_changes(buffer)?;
        Ok(())
    }

    fn apply_changes(&mut self, buffer: &mut Buffer) -> Result<(), Box<dyn Error>> {
        self.0.apply_changes(buffer)?;
        Ok(())
    }
}

impl std::fmt::Display for CommandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#command:{}", self.0)
    }
}

#[allow(unused_variables)]
pub trait Command: Clone + Sized + Serialize + DeserializeOwned {
    type Context;

    fn on_do(&mut self, context: &mut Self::Context) -> Result<(), Box<dyn Error>>;

    fn on_undo(&mut self, context: &mut Self::Context) -> Result<(), Box<dyn Error>>;

    fn tick(&self) -> Result<Option<Self>, Box<dyn Error>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(bound = "T: Command")]
pub struct CommandSlot<T: Command> {
    pub id: CommandId,
    pub data: T,
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(bound = "T: Command")]
pub struct CommandFrame<T: Command> {
    pub id_generator: CommandId,
    pub commands: SmallVec<[CommandSlot<T>; 4]>,
}

impl<T: Command> Default for CommandFrame<T> {
    fn default() -> Self {
        Self {
            id_generator: Default::default(),
            commands: Default::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(bound = "T: Command")]
pub struct CommandHistoryBuffer<T: Command> {
    history: HistoryBuffer<CommandFrame<T>>,
}

impl<T: Command> CommandHistoryBuffer<T> {
    pub fn with_capacity(capacity: usize) -> Self {
        let mut history = HistoryBuffer::with_capacity(capacity);
        history.set(TimeStamp::new(0), Default::default());
        Self { history }
    }

    pub fn without_capacity() -> Self {
        let mut history = HistoryBuffer::without_capacity();
        history.set(TimeStamp::new(0), Default::default());
        Self { history }
    }

    pub fn clone_to(&self, other: &mut Self, range: impl RangeBounds<TimeStamp> + Clone) {
        self.history.clone_to(&mut other.history, range);
    }

    pub fn capacity(&self) -> usize {
        self.history.capacity()
    }

    pub fn set_capacity(&mut self, capacity: usize) {
        self.history.set_capacity(capacity);
    }

    pub fn set_range(&mut self, range: impl RangeBounds<TimeStamp>) -> Result<(), Box<dyn Error>> {
        self.history.set_range(range);
        self.tick_once(self.history.now().unwrap_or_default())?;
        Ok(())
    }

    pub fn now(&self) -> Option<TimeStamp> {
        self.history.now()
    }

    pub fn past(&self) -> Option<TimeStamp> {
        self.history.past()
    }

    pub fn range(&self) -> Option<RangeInclusive<TimeStamp>> {
        self.history.range()
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn iter(
        &self,
        range: impl RangeBounds<TimeStamp> + Clone,
    ) -> impl Iterator<Item = (TimeStamp, &CommandFrame<T>)> {
        self.history.iter(range)
    }

    pub fn frame(&self, timestamp: TimeStamp) -> Option<&CommandFrame<T>> {
        self.history.get(timestamp)
    }

    pub fn command(&self, timestamp: TimeStamp, id: CommandId) -> Option<&T> {
        self.history
            .get(timestamp)
            .and_then(|frame| frame.commands.iter().find(|c| c.id == id))
            .map(|command| &command.data)
    }

    pub fn commands(&self, timestamp: TimeStamp) -> impl Iterator<Item = &T> {
        self.history
            .get(timestamp)
            .into_iter()
            .flat_map(|frame| frame.commands.iter().map(|command| &command.data))
    }

    pub fn clear(&mut self) {
        self.history.clear();
        self.history.set(TimeStamp::new(0), Default::default());
    }

    pub fn has(&self, timestamp: TimeStamp) -> bool {
        self.history.has(timestamp)
    }

    pub fn has_command(&self, timestamp: TimeStamp, id: CommandId) -> bool {
        self.history
            .get(timestamp)
            .map(|frame| frame.commands.iter().any(|c| c.id == id))
            .unwrap_or(false)
    }

    pub fn divergence(&self, other: &Self) -> Option<TimeStamp>
    where
        T: PartialEq,
    {
        self.history.divergence(&other.history)
    }

    pub fn spawn(&mut self, timestamp: TimeStamp, command: T) -> Result<CommandId, Box<dyn Error>> {
        let Some(frame) = self.history.get_mut(timestamp) else {
            return Err(format!("No frame found for tick: {:?}", timestamp).into());
        };
        let id = frame.id_generator;
        frame.id_generator.0 += 1;
        frame.commands.push(CommandSlot { id, data: command });
        Ok(id)
    }

    pub fn do_once(
        &mut self,
        context: &mut T::Context,
        timestamp: TimeStamp,
    ) -> Result<(), Box<dyn Error>> {
        let Some(frame) = self.history.get_mut(timestamp) else {
            return Err(format!("No frame found for tick: {:?}", timestamp).into());
        };
        for command in &mut frame.commands {
            command.data.on_do(context).ok();
        }
        Ok(())
    }

    pub fn do_range(
        &mut self,
        context: &mut T::Context,
        range: impl RangeBounds<TimeStamp>,
    ) -> Result<(), Box<dyn Error>> {
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
        .min(self.now().unwrap_or_default());
        for tick in range_start.ticks()..range_end.ticks() {
            let current_tick = TimeStamp::new(tick);
            self.do_once(context, current_tick)?;
        }
        Ok(())
    }

    pub fn undo_once(
        &mut self,
        context: &mut T::Context,
        timestamp: TimeStamp,
    ) -> Result<(), Box<dyn Error>> {
        let Some(frame) = self.history.get_mut(timestamp) else {
            return Err(format!("No frame found for tick: {:?}", timestamp).into());
        };
        for command in frame.commands.iter_mut().rev() {
            command.data.on_undo(context).ok();
        }
        Ok(())
    }

    pub fn undo_range(
        &mut self,
        context: &mut T::Context,
        range: impl RangeBounds<TimeStamp>,
    ) -> Result<(), Box<dyn Error>> {
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
        .min(self.now().unwrap_or_default());
        for tick in (range_start.ticks()..range_end.ticks()).rev() {
            let current_tick = TimeStamp::new(tick);
            self.undo_once(context, current_tick)?;
        }
        Ok(())
    }

    pub fn tick_once(&mut self, timestamp: TimeStamp) -> Result<(), Box<dyn Error>> {
        let Some(frame) = self.history.get_mut(timestamp) else {
            return Err(format!("No frame found for tick: {:?}", timestamp).into());
        };
        let mut commands = Vec::new();
        for command in &mut frame.commands {
            if let Some(new_command) = command.data.tick()? {
                commands.push(CommandSlot {
                    id: command.id,
                    data: new_command,
                });
            }
        }
        let frame = CommandFrame {
            id_generator: frame.id_generator,
            commands: commands.into(),
        };
        self.history.set(timestamp + 1, frame);
        Ok(())
    }

    pub fn tick_range(&mut self, range: impl RangeBounds<TimeStamp>) -> Result<(), Box<dyn Error>> {
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
        };
        for tick in range_start.ticks()..=range_end.ticks() {
            let current_tick = TimeStamp::new(tick);
            self.tick_once(current_tick)?;
        }
        Ok(())
    }

    pub fn time_travel_to(&mut self, timestamp: TimeStamp) -> Result<(), Box<dyn Error>> {
        self.history.time_travel_to(timestamp);
        self.tick_once(self.history.now().unwrap_or_default())?;
        Ok(())
    }

    pub fn ensure_timestamp(&mut self, timestamp: TimeStamp) -> Result<(), Box<dyn Error>> {
        let now = self.history.now().unwrap_or_default();
        if timestamp > now {
            self.tick_range(now..=timestamp)?;
        }
        Ok(())
    }

    pub fn collect_history(
        &self,
        range: impl RangeBounds<TimeStamp> + Clone,
    ) -> Option<CommandHistoryEvent<T>> {
        let mut start = None;
        let mut snapshots = SmallVec::new();
        for (timestamp, snapshot) in self.iter(range) {
            if start.is_none() {
                start = Some(timestamp);
            }
            snapshots.push(CommandSnapshot::Full(snapshot.clone()));
        }
        Some(CommandHistoryEvent {
            start: start?,
            snapshots,
        })
    }

    // TODO: delta compression has a bug where in weird case it might repeat
    // action over and over for ever. It's either here or on apply - INVESTIGATE!
    pub fn collect_history_delta(
        &self,
        range: impl RangeBounds<TimeStamp> + Clone,
    ) -> Option<CommandHistoryEvent<T>> {
        let mut start = None;
        let mut snapshots = SmallVec::new();
        let mut last_snapshot: Option<&CommandFrame<T>> = None;
        for (timestamp, snapshot) in self.iter(range) {
            if start.is_none() {
                start = Some(timestamp);
                snapshots.push(CommandSnapshot::Full(snapshot.clone()));
            } else {
                let prev = last_snapshot.as_ref().unwrap();
                if prev.id_generator == snapshot.id_generator
                    && prev.commands.len() == snapshot.commands.len()
                    && prev
                        .commands
                        .iter()
                        .zip(&snapshot.commands)
                        .all(|(a, b)| a.id == b.id)
                {
                    snapshots.push(CommandSnapshot::SameAsLast);
                } else {
                    let mut added = SmallVec::new();
                    let mut removed = SmallVec::new();
                    for command in &snapshot.commands {
                        if prev.commands.iter().all(|c| c.id != command.id) {
                            added.push(command.clone());
                        }
                    }
                    for command in &prev.commands {
                        if snapshot.commands.iter().all(|c| c.id != command.id) {
                            removed.push(command.id);
                        }
                    }
                    snapshots.push(CommandSnapshot::DifferentFromLast {
                        id_generator: snapshot.id_generator,
                        added,
                        removed,
                    });
                }
            }
            last_snapshot = Some(snapshot);
        }
        Some(CommandHistoryEvent {
            start: start?,
            snapshots,
        })
    }

    pub fn collect_snapshot(&self, timestamp: TimeStamp) -> Option<CommandHistoryEvent<T>> {
        let snapshot = CommandSnapshot::Full(self.history.get(timestamp)?.clone());
        Some(CommandHistoryEvent {
            start: timestamp,
            snapshots: [snapshot].into_iter().collect(),
        })
    }

    pub fn apply_history(&mut self, event: &CommandHistoryEvent<T>) -> Result<(), Box<dyn Error>> {
        self.history.set_range(..=event.now());
        let mut timestamp = event.start;
        for snapshot in &event.snapshots {
            match snapshot {
                CommandSnapshot::Full(frame) => {
                    self.history.set(timestamp, frame.clone());
                }
                CommandSnapshot::SameAsLast => {
                    let Some(prev) = self.history.get(timestamp - 1) else {
                        return Err(
                            format!("There is no previous frame: {:?}", timestamp - 1).into()
                        );
                    };
                    let mut frame = CommandFrame {
                        id_generator: prev.id_generator,
                        commands: Default::default(),
                    };
                    for command in &prev.commands {
                        if let Some(new_command) = command.data.tick()? {
                            frame.commands.push(CommandSlot {
                                id: command.id,
                                data: new_command,
                            });
                        }
                    }
                    self.history.set(timestamp, frame);
                }
                CommandSnapshot::DifferentFromLast {
                    id_generator,
                    added,
                    removed,
                } => {
                    let Some(prev) = self.history.get(timestamp - 1) else {
                        return Err(
                            format!("There is no previous frame: {:?}", timestamp - 1).into()
                        );
                    };
                    let mut frame = CommandFrame {
                        id_generator: *id_generator,
                        commands: prev.commands.clone(),
                    };
                    frame.id_generator = *id_generator;
                    for command in added {
                        if !frame.commands.iter().any(|c| c.id == command.id) {
                            frame.commands.push(command.clone());
                        }
                    }
                    for id in removed {
                        frame.commands.retain(|slot| slot.id != *id);
                    }
                    self.history.set(timestamp, frame);
                }
            }
            timestamp += 1;
        }
        Ok(())
    }

    pub fn apply_history_divergence(
        &mut self,
        event: &CommandHistoryEvent<T>,
    ) -> Result<Option<TimeStamp>, Box<dyn Error>>
    where
        T: Clone + PartialEq,
    {
        // TODO: optimize!
        let mut temp = Self::with_capacity(event.snapshots.len());
        temp.apply_history(event)?;
        let divergence = self.divergence(&temp);
        temp.clone_to(self, ..);
        Ok(divergence)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "T: Command")]
pub enum CommandSnapshot<T: Command> {
    Full(CommandFrame<T>),
    SameAsLast,
    DifferentFromLast {
        id_generator: CommandId,
        added: SmallVec<[CommandSlot<T>; 4]>,
        removed: SmallVec<[CommandId; 4]>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound = "T: Command")]
pub struct CommandHistoryEvent<T: Command> {
    start: TimeStamp,
    snapshots: SmallVec<[CommandSnapshot<T>; 4]>,
}

impl<T: Command> CommandHistoryEvent<T> {
    pub fn new(start: TimeStamp, snapshots: impl IntoIterator<Item = CommandSnapshot<T>>) -> Self {
        Self {
            start,
            snapshots: snapshots.into_iter().collect(),
        }
    }

    pub fn single(timestamp: TimeStamp, snapshot: CommandFrame<T>) -> Self {
        Self {
            start: timestamp,
            snapshots: [CommandSnapshot::Full(snapshot)].into_iter().collect(),
        }
    }

    pub fn now(&self) -> TimeStamp {
        self.start + self.snapshots.len() as u64 - 1
    }

    pub fn past(&self) -> TimeStamp {
        self.start
    }

    pub fn range(&self) -> RangeInclusive<TimeStamp> {
        self.past()..=self.now()
    }

    pub fn iter(&self) -> impl Iterator<Item = (TimeStamp, &CommandSnapshot<T>)> {
        self.snapshots
            .iter()
            .enumerate()
            .map(move |(index, snapshot)| (self.start + index as u64, snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Change {
        Do(usize),
        Undo(usize),
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Counter {
        index: usize,
        count: usize,
    }

    impl Counter {
        fn new(count: usize) -> Self {
            Self {
                index: 0,
                count: count.max(1),
            }
        }
    }

    impl Command for Counter {
        type Context = Vec<Change>;

        fn tick(&self) -> Result<Option<Self>, Box<dyn Error>> {
            let index = self.index + 1;
            if index < self.count {
                Ok(Some(Self {
                    index,
                    count: self.count,
                }))
            } else {
                Ok(None)
            }
        }

        fn on_do(&mut self, context: &mut Self::Context) -> Result<(), Box<dyn Error>> {
            context.push(Change::Do(self.index));
            Ok(())
        }

        fn on_undo(&mut self, context: &mut Self::Context) -> Result<(), Box<dyn Error>> {
            context.push(Change::Undo(self.index));
            Ok(())
        }
    }

    #[test]
    fn test_command_progression() {
        let mut changes = Vec::<Change>::new();
        let mut history = CommandHistoryBuffer::without_capacity();
        history.spawn(TimeStamp::new(0), Counter::new(3)).unwrap();

        history
            .tick_range(TimeStamp::new(0)..TimeStamp::new(5))
            .unwrap();
        assert_eq!(history.len(), 6);
        assert_eq!(history.now(), Some(TimeStamp::new(5)));
        assert_eq!(history.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            history.history.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 0, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(1),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 1, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(2),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 2, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(3),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                ),
                (
                    TimeStamp::new(4),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                ),
                (
                    TimeStamp::new(5),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                )
            ]
        );

        changes.clear();
        history.do_range(&mut changes, ..).unwrap();
        assert_eq!(changes, vec![Change::Do(0), Change::Do(1), Change::Do(2)]);

        changes.clear();
        history
            .undo_range(&mut changes, TimeStamp::new(1)..)
            .unwrap();
        assert_eq!(changes, vec![Change::Undo(2), Change::Undo(1)]);

        history.time_travel_to(TimeStamp::new(1)).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history.now(), Some(TimeStamp::new(2)));
        assert_eq!(history.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            history.history.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 0, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(1),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 1, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(2),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 2, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                )
            ]
        );

        history
            .tick_range(TimeStamp::new(1)..=TimeStamp::new(2))
            .unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history.now(), Some(TimeStamp::new(3)));
        assert_eq!(history.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            history.history.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 0, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(1),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 1, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(2),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 2, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(3),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                )
            ]
        );

        changes.clear();
        history.do_range(&mut changes, TimeStamp::new(1)..).unwrap();
        assert_eq!(changes, vec![Change::Do(1), Change::Do(2)]);

        changes.clear();
        history
            .undo_range(&mut changes, TimeStamp::new(2)..)
            .unwrap();
        assert_eq!(changes, vec![Change::Undo(2)]);

        history.set_range(..=TimeStamp::new(1)).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history.now(), Some(TimeStamp::new(2)));
        assert_eq!(history.past(), Some(TimeStamp::new(0)));
        assert_eq!(
            history.history.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 0, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(1),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 1, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(2),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 2, count: 3 }
                        }]
                        .into_iter()
                        .collect(),
                    }
                )
            ]
        );
    }

    #[test]
    fn test_command_event() {
        let mut history = CommandHistoryBuffer::without_capacity();
        history.spawn(TimeStamp::new(0), Counter::new(3)).unwrap();
        history
            .tick_range(TimeStamp::new(0)..TimeStamp::new(5))
            .unwrap();

        let event = history.collect_history_delta(..).unwrap();
        assert_eq!(event.start, TimeStamp::new(0));
        assert_eq!(
            event.snapshots,
            SmallVec::from_buf([
                CommandSnapshot::Full(CommandFrame {
                    id_generator: CommandId::new(1),
                    commands: [CommandSlot {
                        id: CommandId::new(0),
                        data: Counter { index: 0, count: 3 },
                    }]
                    .into_iter()
                    .collect(),
                }),
                CommandSnapshot::SameAsLast,
                CommandSnapshot::SameAsLast,
                CommandSnapshot::DifferentFromLast {
                    id_generator: CommandId::new(1),
                    added: Default::default(),
                    removed: [CommandId::new(0)].into_iter().collect(),
                },
                CommandSnapshot::SameAsLast,
                CommandSnapshot::SameAsLast,
            ])
        );

        let mut new_history = CommandHistoryBuffer::without_capacity();
        new_history.apply_history(&event).unwrap();
        assert_eq!(history, new_history);

        let event = history.collect_history_delta(TimeStamp::new(2)..).unwrap();
        let mut history = CommandHistoryBuffer::without_capacity();
        history.tick_range(..=TimeStamp::new(1)).unwrap();
        history.spawn(TimeStamp::new(1), Counter::new(5)).unwrap();
        history
            .tick_range(TimeStamp::new(1)..=TimeStamp::new(2))
            .unwrap();
        let divergence = history.apply_history_divergence(&event).unwrap().unwrap();
        assert_eq!(divergence, TimeStamp::new(2));
        assert_eq!(
            history.history.iter(..).collect::<Vec<_>>(),
            vec![
                (
                    TimeStamp::new(0),
                    &CommandFrame {
                        id_generator: CommandId::new(0),
                        commands: Default::default(),
                    }
                ),
                (
                    TimeStamp::new(1),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 0, count: 5 },
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(2),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: [CommandSlot {
                            id: CommandId::new(0),
                            data: Counter { index: 2, count: 3 },
                        }]
                        .into_iter()
                        .collect(),
                    }
                ),
                (
                    TimeStamp::new(3),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                ),
                (
                    TimeStamp::new(4),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                ),
                (
                    TimeStamp::new(5),
                    &CommandFrame {
                        id_generator: CommandId::new(1),
                        commands: Default::default(),
                    }
                ),
            ]
        );
    }
}
