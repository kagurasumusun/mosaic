//! Clock state vocabulary. Clock-tree resolution belongs to runtime.
//!
//! Functional models ignore sub-picosecond edges and cross-domain jitter
//! (設計書 §13.2). [`PS_PER_SECOND`] converts a frequency in hertz into
//! virtual-time picoseconds.

use core::num::NonZeroU64;

/// Picoseconds in one SI second. Virtual time uses this resolution (ADR 0003).
pub const PS_PER_SECOND: u64 = 1_000_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClockState {
    Gated,
    Running { hz: NonZeroU64 },
}

pub trait ClockSink: Send {
    fn clock_changed(&mut self, state: ClockState);
}
