#![no_std]
#![forbid(unsafe_code)]
//! Provisional synchronous model context. More services require an ADR.

use mosaic_core_types::{EventId, SignalId, VTime};

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

    /// Checked relative scheduling; zero delay is valid, overflow is an error.
    fn schedule_after(&mut self, delay: VTime, token: u64) -> Result<EventId, ContextError> {
        let at = self
            .now()
            .checked_add(delay)
            .ok_or(ContextError::TimeOverflow)?;
        self.schedule(at, token)
    }

    /// Cancel only this node's pending event; false for stale or foreign IDs.
    fn cancel(&mut self, event: EventId) -> bool;

    /// Queue an outgoing signal at now. Never invoke its sink synchronously.
    fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError>;
}

/// Receives node-local timer events after the runtime pops them from the queue.
///
/// A model may implement this alongside its bus and signal contracts. The
/// runtime calls it only between queue pops, never inside another callback, so
/// effects produced through [`Ctx`] stay deferred and cannot re-enter the model.
/// Timer tokens are model-defined values from the model's own reservations.
pub trait EventTarget: Send {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError>;
}
