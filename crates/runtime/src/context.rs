use std::collections::BTreeSet;

use mosaic_contract::model::{ContextError, Ctx};
use mosaic_exec::{EventId, NodeId, SignalId, VTime};

use crate::{EventKind, EventQueue};

/// Callback-scoped access to runtime services, bound to one node's identity.
///
/// `signals` contains only the outgoing signal IDs assigned to this node by the
/// assembler. It is not a list of destinations or references to other models.
/// Dropping the context releases the queue so the dispatcher can pop effects.
/// The context itself never dispatches: no nested callback can occur here.
pub struct ModelContext<'a> {
    queue: &'a mut EventQueue,
    owner: NodeId,
    signals: &'a BTreeSet<u32>,
}

impl<'a> ModelContext<'a> {
    pub fn new(queue: &'a mut EventQueue, owner: NodeId, signals: &'a BTreeSet<u32>) -> Self {
        Self {
            queue,
            owner,
            signals,
        }
    }
}

impl Ctx for ModelContext<'_> {
    fn now(&self) -> VTime {
        self.queue.now()
    }

    fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
        self.queue
            .schedule(self.owner, at, EventKind::Timer { token })
    }

    fn cancel(&mut self, event: EventId) -> bool {
        self.queue.cancel(self.owner, event)
    }

    fn reschedule(
        &mut self,
        event: EventId,
        at: VTime,
        token: u64,
    ) -> Result<EventId, ContextError> {
        self.queue
            .reschedule(self.owner, event, at, EventKind::Timer { token })
    }

    fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
        if !self.signals.contains(&signal.0) {
            return Err(ContextError::UnknownSignal);
        }
        self.queue.schedule(
            self.owner,
            self.queue.now(),
            EventKind::Signal { signal, high },
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{Snapshot, VDelta};

    #[test]
    fn runtime_handles_preserve_the_send_contract() {
        fn assert_send<T: Send>() {}
        assert_send::<EventQueue>();
        assert_send::<ModelContext<'static>>();
    }

    #[test]
    fn context_uses_virtual_time_and_node_local_timer_identity() {
        let mut queue = EventQueue::new(4);
        queue.pop_due(VTime(10)).unwrap();
        let signals = BTreeSet::new();
        let id = {
            let mut context = ModelContext::new(&mut queue, NodeId(3), &signals);
            let ctx: &mut dyn Ctx = &mut context; // Verify object safety.
            assert_eq!(ctx.now(), VTime(10));
            ctx.schedule_after(VDelta(5), 42).unwrap()
        };
        let event = queue.pop_next().unwrap();
        assert_eq!(event.id, id);
        assert_eq!(event.at, VTime(15));
        assert_eq!(event.owner, NodeId(3));
        assert_eq!(event.kind, EventKind::Timer { token: 42 });
    }

    #[test]
    fn contexts_cannot_cancel_other_nodes_timers() {
        let mut queue = EventQueue::new(4);
        let signals = BTreeSet::new();
        let id = ModelContext::new(&mut queue, NodeId(0), &signals)
            .schedule(VTime(10), 1)
            .unwrap();
        assert!(!ModelContext::new(&mut queue, NodeId(1), &signals).cancel(id));
        assert!(ModelContext::new(&mut queue, NodeId(0), &signals).cancel(id));
        assert!(queue.is_empty());
    }

    #[test]
    fn signal_changes_are_deferred_and_keep_order_with_same_time_timers() {
        let mut queue = EventQueue::new(4);
        let signals = BTreeSet::from([7]);
        {
            let mut context = ModelContext::new(&mut queue, NodeId(2), &signals);
            context.set_signal(SignalId(7), true).unwrap();
            context.schedule(VTime::ZERO, 1).unwrap();
            context.set_signal(SignalId(7), false).unwrap();
            assert_eq!(context.now(), VTime::ZERO);
        }
        // Nothing has been delivered while the callback's context was borrowed.
        assert_eq!(queue.len(), 3);
        assert_eq!(
            queue.pop_next().unwrap().kind,
            EventKind::Signal {
                signal: SignalId(7),
                high: true
            }
        );
        assert_eq!(
            queue.pop_next().unwrap().kind,
            EventKind::Timer { token: 1 }
        );
        assert_eq!(
            queue.pop_next().unwrap().kind,
            EventKind::Signal {
                signal: SignalId(7),
                high: false
            }
        );
    }

    #[test]
    fn unknown_signal_is_rejected_without_mutation() {
        let mut queue = EventQueue::new(4);
        let before = queue.save();
        let signals = BTreeSet::from([7]);
        let mut context = ModelContext::new(&mut queue, NodeId(0), &signals);
        assert_eq!(
            context.set_signal(SignalId(8), true),
            Err(ContextError::UnknownSignal)
        );
        assert_eq!(queue.save(), before);
    }

    #[test]
    fn full_queue_rejects_signal_without_mutation() {
        let mut queue = EventQueue::new(0);
        let before = queue.save();
        let signals = BTreeSet::from([7]);
        let mut context = ModelContext::new(&mut queue, NodeId(0), &signals);
        assert_eq!(
            context.set_signal(SignalId(7), true),
            Err(ContextError::ResourceExhausted)
        );
        assert_eq!(queue.save(), before);
    }

    #[test]
    fn relative_time_overflow_is_not_saturated_or_wrapped() {
        let mut queue = EventQueue::new(1);
        queue.pop_due(VTime(u64::MAX)).unwrap();
        let before = queue.save();
        let signals = BTreeSet::new();
        let mut context = ModelContext::new(&mut queue, NodeId(0), &signals);
        assert_eq!(
            context.schedule_after(VDelta(1), 0),
            Err(ContextError::TimeOverflow)
        );
        assert_eq!(queue.save(), before);
    }
}
