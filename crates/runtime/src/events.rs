use std::collections::BTreeMap;

use mosaic_contract::model::ContextError;
use mosaic_exec::{EventId, Fnv64, NodeId, SignalId, Snapshot, SnapshotError, VTime};

/// A node-local timer or an outgoing logical signal. Wiring is resolved later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EventKind {
    Timer { token: u64 },
    Signal { signal: SignalId, high: bool },
}

/// IDs are monotonically allocated and also define insertion order at a time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub id: EventId,
    pub at: VTime,
    pub owner: NodeId,
    pub kind: EventKind,
}

/// Typed state only, not a persistent file format or a whole-machine snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueState {
    pub now: VTime,
    pub next_id: u64,
    pub events: Vec<Event>,
}

/// Event order is (virtual time, insertion ID), independent of node identity.
///
/// A capacity bounds pending events, including signal changes. IDs are never
/// reused after dispatch or cancellation. Counter exhaustion is an error rather
/// than wraparound. Snapshot load replaces the timeline, including its ID space;
/// callers must discard handles from the old timeline when restoring.
#[derive(Debug)]
pub struct EventQueue {
    now: VTime,
    next_id: u64,
    capacity: usize,
    events: BTreeMap<(VTime, u64), Event>,
    times: BTreeMap<u64, VTime>,
}

impl EventQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            now: VTime::ZERO,
            next_id: 0,
            capacity,
            events: BTreeMap::new(),
            times: BTreeMap::new(),
        }
    }

    pub fn now(&self) -> VTime {
        self.now
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn next_time(&self) -> Option<VTime> {
        self.events.first_key_value().map(|((at, _), _)| *at)
    }

    /// Schedule without advancing time or invoking any model callback.
    /// A failed reservation leaves both the queue and ID counter unchanged.
    pub fn schedule(
        &mut self,
        owner: NodeId,
        at: VTime,
        kind: EventKind,
    ) -> Result<EventId, ContextError> {
        if at < self.now {
            return Err(ContextError::TimeInPast);
        }
        if self.len() >= self.capacity {
            return Err(ContextError::ResourceExhausted);
        }
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ContextError::ResourceExhausted)?;
        let id = EventId(self.next_id);
        let event = Event {
            id,
            at,
            owner,
            kind,
        };
        self.events.insert((at, id.0), event);
        self.times.insert(id.0, at);
        self.next_id = next_id;
        Ok(id)
    }

    /// Atomically replace one pending event owned by `owner`.
    ///
    /// If the ID is stale or belongs to another node, this behaves like a new
    /// reservation. Replacing a valid event does not require an extra queue
    /// slot, and validation/counter exhaustion failures leave the old event
    /// untouched.
    pub fn reschedule(
        &mut self,
        owner: NodeId,
        id: EventId,
        at: VTime,
        kind: EventKind,
    ) -> Result<EventId, ContextError> {
        if at < self.now {
            return Err(ContextError::TimeInPast);
        }
        let old_at = self.times.get(&id.0).copied();
        let old_event = old_at.and_then(|old_at| self.events.get(&(old_at, id.0)).copied());
        if old_event.is_none_or(|event| event.owner != owner) {
            return self.schedule(owner, at, kind);
        }

        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(ContextError::ResourceExhausted)?;
        let replacement_id = EventId(self.next_id);
        let replacement = Event {
            id: replacement_id,
            at,
            owner,
            kind,
        };
        let old_at = old_at.expect("a validated old event has a timestamp");
        self.events.remove(&(old_at, id.0));
        self.times.remove(&id.0);
        self.events.insert((at, replacement_id.0), replacement);
        self.times.insert(replacement_id.0, at);
        self.next_id = next_id;
        Ok(replacement_id)
    }

    /// A node can only cancel its own pending events, including signal changes.
    pub fn cancel(&mut self, owner: NodeId, id: EventId) -> bool {
        let Some(&at) = self.times.get(&id.0) else {
            return false;
        };
        let key = (at, id.0);
        if !self.events.get(&key).is_some_and(|e| e.owner == owner) {
            return false;
        }
        self.events.remove(&key);
        self.times.remove(&id.0);
        true
    }

    /// Pop one event and advance to its time. Dispatch is the caller's job.
    /// An empty queue leaves time unchanged.
    pub fn pop_next(&mut self) -> Option<Event> {
        let (_, event) = self.events.pop_first()?;
        self.times.remove(&event.id.0);
        self.now = event.at;
        Some(event)
    }

    /// Pop one event at or before `until`. If none is due, advance to `until`.
    ///
    /// Call repeatedly until None to drain a time window. Even events exactly
    /// at the boundary (including events subsequently scheduled there) are due.
    /// This API never skips a queued event. Backwards time is rejected without
    /// mutation. Like any event loop, endless same-time rescheduling must be
    /// bounded by the caller's execution budget.
    pub fn pop_due(&mut self, until: VTime) -> Result<Option<Event>, ContextError> {
        if until < self.now {
            return Err(ContextError::TimeInPast);
        }
        if self.next_time().is_some_and(|at| at <= until) {
            return Ok(self.pop_next());
        }
        self.now = until;
        Ok(None)
    }

    /// Commit a state that has already passed `Snapshot::validate`.
    pub(crate) fn restore_validated(&mut self, state: &QueueState) {
        let mut events = BTreeMap::new();
        let mut times = BTreeMap::new();
        for event in &state.events {
            events.insert((event.at, event.id.0), *event);
            times.insert(event.id.0, event.at);
        }
        self.now = state.now;
        self.next_id = state.next_id;
        self.events = events;
        self.times = times;
    }
}

impl Snapshot for EventQueue {
    type State = QueueState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        QueueState {
            now: self.now,
            next_id: self.next_id,
            events: self.events.values().copied().collect(),
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.events.len() > self.capacity {
            return Err(SnapshotError::InvalidState);
        }
        let mut ids = BTreeMap::new();
        for event in &state.events {
            if event.at < state.now
                || event.id.0 >= state.next_id
                || ids.insert(event.id.0, event.at).is_some()
            {
                return Err(SnapshotError::InvalidState);
            }
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.restore_validated(state);
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.u64(state.now.0);
        hash.u64(state.next_id);
        hash.usize(state.events.len());
        for event in &state.events {
            hash.u64(event.id.0);
            hash.u64(event.at.0);
            hash.u32(event.owner.0);
            match event.kind {
                EventKind::Timer { token } => {
                    hash.byte(1);
                    hash.u64(token);
                }
                EventKind::Signal { signal, high } => {
                    hash.byte(2);
                    hash.u32(signal.0);
                    hash.bool(high);
                }
            }
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(queue: &mut EventQueue, node: u32, at: u64, token: u64) -> EventId {
        queue
            .schedule(NodeId(node), VTime(at), EventKind::Timer { token })
            .unwrap()
    }

    fn drain(queue: &mut EventQueue) -> Vec<Event> {
        std::iter::from_fn(|| queue.pop_next()).collect()
    }

    #[test]
    fn events_are_time_ordered_and_ties_keep_insertion_order() {
        let mut queue = EventQueue::new(4);
        let last = timer(&mut queue, 0, 30, 1);
        let first = timer(&mut queue, 9, 10, 2);
        let second = timer(&mut queue, 1, 10, 3);
        let third = timer(&mut queue, 0, 20, 4);
        assert_eq!(queue.next_time(), Some(VTime(10)));
        let ids: Vec<_> = drain(&mut queue).iter().map(|e| e.id).collect();
        assert_eq!(ids, [first, second, third, last]);
        assert_eq!(queue.now(), VTime(30));
        assert_eq!(queue.next_time(), None);
        assert_eq!(queue.pop_next(), None);
        assert_eq!(queue.now(), VTime(30));
    }

    #[test]
    fn reservation_does_not_advance_time() {
        let mut queue = EventQueue::new(1);
        timer(&mut queue, 0, u64::MAX, 0);
        assert_eq!(queue.now(), VTime::ZERO);
        assert_eq!(queue.pop_next().unwrap().at, VTime(u64::MAX));
    }

    #[test]
    fn past_reservation_is_rejected_without_mutation() {
        let mut queue = EventQueue::new(2);
        assert_eq!(queue.pop_due(VTime(10)), Ok(None));
        let before = queue.save();
        assert_eq!(
            queue.schedule(NodeId(0), VTime(9), EventKind::Timer { token: 0 }),
            Err(ContextError::TimeInPast)
        );
        assert_eq!(queue.save(), before);
        timer(&mut queue, 0, 10, 0); // Scheduling at now is valid.
    }

    #[test]
    fn rescheduling_at_capacity_replaces_the_old_event_atomically() {
        let mut queue = EventQueue::new(1);
        let old = timer(&mut queue, 3, 10, 1);
        let replacement = queue
            .reschedule(NodeId(3), old, VTime(20), EventKind::Timer { token: 2 })
            .unwrap();

        assert_eq!(queue.len(), 1);
        assert_ne!(replacement, old);
        assert_eq!(queue.next_time(), Some(VTime(20)));
        assert_eq!(
            queue.pop_next().unwrap().kind,
            EventKind::Timer { token: 2 }
        );
    }

    #[test]
    fn failed_reschedule_keeps_the_old_event_and_timeline() {
        let mut queue = EventQueue::new(1);
        let old = timer(&mut queue, 3, 20, 1);
        // Advance now without consuming the future event, then reject a
        // replacement that would move it backwards.
        assert_eq!(queue.pop_due(VTime(10)), Ok(None));
        let before = queue.save();
        assert_eq!(
            queue.reschedule(NodeId(3), old, VTime(9), EventKind::Timer { token: 2 }),
            Err(ContextError::TimeInPast)
        );
        assert_eq!(queue.save(), before);
        assert_eq!(queue.next_time(), Some(VTime(20)));
    }

    #[test]
    fn capacity_failure_does_not_consume_an_id() {
        let mut queue = EventQueue::new(1);
        let first = timer(&mut queue, 0, 1, 0);
        let before = queue.save();
        assert_eq!(
            queue.schedule(NodeId(0), VTime(2), EventKind::Timer { token: 1 }),
            Err(ContextError::ResourceExhausted)
        );
        assert_eq!(queue.save(), before);
        assert!(queue.cancel(NodeId(0), first));
        assert_eq!(timer(&mut queue, 0, 2, 1).0, first.0 + 1);
    }

    #[test]
    fn zero_capacity_is_well_defined() {
        let mut queue = EventQueue::new(0);
        assert_eq!(
            queue.schedule(NodeId(0), VTime::ZERO, EventKind::Timer { token: 0 }),
            Err(ContextError::ResourceExhausted)
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn cancellation_is_owner_scoped_and_idempotent() {
        let mut queue = EventQueue::new(2);
        let id = timer(&mut queue, 2, 10, 0);
        let before = queue.save();
        assert!(!queue.cancel(NodeId(1), id));
        assert_eq!(queue.save(), before);
        assert!(queue.cancel(NodeId(2), id));
        assert!(!queue.cancel(NodeId(2), id));
        assert!(!queue.cancel(NodeId(2), EventId(u64::MAX)));
        assert!(queue.is_empty());
        let next = timer(&mut queue, 2, 10, 0);
        assert_ne!(id, next);
        queue.pop_next();
        assert!(!queue.cancel(NodeId(2), next));
    }

    #[test]
    fn cancelled_events_are_removed_from_both_indexes() {
        let mut queue = EventQueue::new(3);
        let id = timer(&mut queue, 0, 10, 0);
        timer(&mut queue, 0, 20, 1);
        assert!(queue.cancel(NodeId(0), id));
        assert_eq!(queue.next_time(), Some(VTime(20)));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.save().events.len(), 1);
        assert_eq!(queue.pop_next().unwrap().at, VTime(20));
        assert!(queue.times.is_empty());
    }

    #[test]
    fn time_window_includes_its_boundary_but_never_skips_events() {
        let mut queue = EventQueue::new(3);
        timer(&mut queue, 0, 10, 0);
        timer(&mut queue, 0, 20, 1);
        timer(&mut queue, 0, 30, 2);
        assert_eq!(queue.pop_due(VTime(20)).unwrap().unwrap().at, VTime(10));
        assert_eq!(queue.now(), VTime(10));
        assert_eq!(queue.pop_due(VTime(20)).unwrap().unwrap().at, VTime(20));
        timer(&mut queue, 0, 20, 3);
        assert_eq!(queue.pop_due(VTime(20)).unwrap().unwrap().at, VTime(20));
        assert_eq!(queue.pop_due(VTime(20)), Ok(None));
        assert_eq!(queue.now(), VTime(20));
        assert_eq!(queue.next_time(), Some(VTime(30)));
        let before = queue.save();
        assert_eq!(queue.pop_due(VTime(19)), Err(ContextError::TimeInPast));
        assert_eq!(queue.save(), before);
    }

    #[test]
    fn snapshot_round_trip_preserves_order_cancellation_and_next_id() {
        let mut queue = EventQueue::new(8);
        queue.pop_due(VTime(5)).unwrap();
        let cancelled = timer(&mut queue, 0, 10, 0);
        timer(&mut queue, 1, 20, 1);
        timer(&mut queue, 0, 20, 2);
        assert!(queue.cancel(NodeId(0), cancelled));
        let state = queue.save();
        let mut restored = EventQueue::new(8);
        restored.load(EventQueue::SCHEMA_VERSION, &state).unwrap();
        assert_eq!(restored.save(), state);
        assert_eq!(timer(&mut queue, 0, 20, 3), timer(&mut restored, 0, 20, 3));
        assert_eq!(drain(&mut queue), drain(&mut restored));
        assert_eq!(queue.save(), restored.save());
    }

    #[test]
    fn snapshot_input_order_does_not_change_dispatch_order() {
        let mut queue = EventQueue::new(3);
        timer(&mut queue, 0, 20, 0);
        timer(&mut queue, 1, 10, 1);
        timer(&mut queue, 2, 10, 2);
        let mut state = queue.save();
        state.events.reverse();
        let mut restored = EventQueue::new(3);
        restored.load(1, &state).unwrap();
        assert_eq!(drain(&mut queue), drain(&mut restored));
    }

    #[test]
    fn invalid_snapshots_leave_existing_state_untouched() {
        let mut queue = EventQueue::new(3);
        queue.pop_due(VTime(5)).unwrap();
        timer(&mut queue, 0, 10, 0);
        let valid = queue.save();
        assert_eq!(queue.load(0, &valid), Err(SnapshotError::VersionMismatch));
        assert_eq!(queue.save(), valid);

        let mut past = valid.clone();
        past.events[0].at = VTime(4);
        let mut duplicate = valid.clone();
        duplicate.events.push(duplicate.events[0]);
        let mut unallocated = valid.clone();
        unallocated.events[0].id = EventId(valid.next_id);
        let mut too_many = valid.clone();
        too_many.events = vec![valid.events[0]; 4];
        for invalid in [past, duplicate, unallocated, too_many] {
            assert_eq!(queue.load(1, &invalid), Err(SnapshotError::InvalidState));
            assert_eq!(queue.save(), valid);
        }
    }

    #[test]
    fn exhausted_id_counter_never_wraps() {
        let mut queue = EventQueue::new(1);
        queue
            .load(
                1,
                &QueueState {
                    now: VTime::ZERO,
                    next_id: u64::MAX - 1,
                    events: vec![],
                },
            )
            .unwrap();
        assert_eq!(timer(&mut queue, 0, 1, 0), EventId(u64::MAX - 1));
        queue.pop_next();
        let before = queue.save();
        assert_eq!(
            queue.schedule(NodeId(0), VTime(2), EventKind::Timer { token: 0 }),
            Err(ContextError::ResourceExhausted)
        );
        assert_eq!(queue.save(), before);
    }

    #[test]
    fn identical_inputs_produce_identical_traces() {
        fn scenario() -> (Vec<Event>, QueueState) {
            let mut queue = EventQueue::new(64);
            for n in 0..64 {
                let id = timer(&mut queue, n % 4, u64::from((n * 17) % 11), u64::from(n));
                if n % 3 == 0 {
                    assert!(queue.cancel(NodeId(n % 4), id));
                }
            }
            let trace = drain(&mut queue);
            (trace, queue.save())
        }
        assert_eq!(scenario(), scenario());
    }

    #[test]
    fn time_window_partition_does_not_change_event_trace() {
        let mut queue = EventQueue::new(4);
        for at in [0, 5, 10, 20] {
            timer(&mut queue, 0, at, at);
        }
        let mut partitioned = EventQueue::new(4);
        partitioned.load(1, &queue.save()).unwrap();
        let expected = drain(&mut queue);
        let mut actual = Vec::new();
        for until in [0, 3, 5, 9, 10, 20] {
            while let Some(event) = partitioned.pop_due(VTime(until)).unwrap() {
                actual.push(event);
            }
        }
        assert_eq!(actual, expected);
        assert_eq!(queue.save(), partitioned.save());
    }
}
