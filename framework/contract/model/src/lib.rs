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
    fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError>;
    fn cancel(&mut self, event: EventId) -> bool;
    fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError>;
}
