#![no_std]
#![forbid(unsafe_code)]
//! Clock state vocabulary. Clock-tree resolution belongs to runtime.

use core::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClockState {
    Gated,
    Running { hz: NonZeroU64 },
}

pub trait ClockSink: Send {
    fn clock_changed(&mut self, state: ClockState);
}
