//! Provisional synchronous model context. More services require an ADR.

use mosaic_exec::{EventId, NodeId, SignalId, VDelta, VTime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ContextError {
    TimeInPast,
    TimeOverflow,
    ResourceExhausted,
    UnknownSignal,
}

/// The runtime binds this context to the currently executing node. No model refs.
/// Signal changes are queued, never delivered reentrantly within a callback.
/// DMA, RNG, trace and host ports are deferred until the first vertical slice.
pub trait Ctx: Send {
    fn now(&self) -> VTime;
    /// Reserve a node-local callback at or after now, without dispatching it.
    fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError>;

    /// Checked relative scheduling. `delay` is a duration, never an absolute
    /// timestamp (ADR 0003). Zero delay is valid; overflow is an error and
    /// must not wrap or saturate.
    fn schedule_after(&mut self, delay: VDelta, token: u64) -> Result<EventId, ContextError> {
        let at = self
            .now()
            .checked_add(delay)
            .ok_or(ContextError::TimeOverflow)?;
        self.schedule(at, token)
    }

    /// Cancel only this node's pending event; false for stale or foreign IDs.
    fn cancel(&mut self, event: EventId) -> bool;

    /// Replace a pending timer with a new deadline and token.
    ///
    /// The default implementation schedules first and then cancels the old
    /// event, preserving the old timer if reservation fails. Runtime-backed
    /// contexts override this with an atomic, capacity-neutral replacement.
    fn reschedule(
        &mut self,
        event: EventId,
        at: VTime,
        token: u64,
    ) -> Result<EventId, ContextError> {
        let replacement = self.schedule(at, token)?;
        let _ = self.cancel(event);
        Ok(replacement)
    }

    /// Queue an outgoing signal at now. Never invoke its sink synchronously.
    fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError>;

    /// Queue a signal that belongs to *another* node — a fabric-attached
    /// part (ADR 0032) whose callback runs inside the context of whichever
    /// node drives the fabric. The part names itself as the origin so the
    /// wiring table resolves the output it owns. Contexts that cannot host
    /// foreign origins (capture-only harnesses) report exhaustion and the
    /// caller treats the undelivered edge as a no-op.
    fn set_signal_from(
        &mut self,
        node: mosaic_exec::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        let _ = (node, signal, high);
        Err(ContextError::ResourceExhausted)
    }
}

/// Drive an optional output signal while preserving runtime failures.
///
/// An absent output is a valid composition choice for many peripheral pins,
/// so `UnknownSignal` is treated as an intentionally unwired port. Queue
/// exhaustion and other errors are not swallowed: callers must not claim an
/// interrupt or pin transition was delivered when it was never queued.
pub fn set_optional_signal(
    ctx: &mut dyn Ctx,
    signal: SignalId,
    high: bool,
) -> Result<(), ContextError> {
    match ctx.set_signal(signal, high) {
        Err(ContextError::UnknownSignal) => Ok(()),
        result => result,
    }
}

/// Receives node-local timer events after the runtime pops them from the queue.
///
/// A model may implement this alongside its bus and signal contracts. The
/// runtime calls it only between queue pops, never inside another callback, so
/// effects produced through [`Ctx`] stay deferred and cannot re-enter the model.
/// Timer tokens are model-defined values from the model's own reservations.
pub trait EventTarget: Send {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError>;

    /// Delivery of one wired input signal edge, with the producing node and
    /// its output pin (構成設計書 §13.3: multiple sources, named pins).
    ///
    /// Aggregator models — interrupt controllers, wired-OR nets — override
    /// this to track per-source line levels and assert their own aggregated
    /// outputs through `ctx`, which the runtime binds to this node. Effects
    /// are queued events, never synchronous re-entry (§13.4 rule 1, ADR 0001).
    /// The default no-op mirrors optional ports: an unwired or ignored input
    /// is not a guest fault.
    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        _source: NodeId,
        _signal: SignalId,
        _high: bool,
    ) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    struct SignalCtx(Result<(), ContextError>);

    impl Ctx for SignalCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }

        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            self.0
        }
    }

    #[test]
    fn optional_signal_ignores_only_unwired_outputs() {
        assert_eq!(
            set_optional_signal(
                &mut SignalCtx(Err(ContextError::UnknownSignal)),
                SignalId(1),
                true,
            ),
            Ok(())
        );
        assert_eq!(
            set_optional_signal(
                &mut SignalCtx(Err(ContextError::ResourceExhausted)),
                SignalId(1),
                true,
            ),
            Err(ContextError::ResourceExhausted)
        );
        assert_eq!(
            set_optional_signal(
                &mut SignalCtx(Err(ContextError::TimeOverflow)),
                SignalId(1),
                true
            ),
            Err(ContextError::TimeOverflow)
        );
    }
}
