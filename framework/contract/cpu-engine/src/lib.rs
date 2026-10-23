#![no_std]
#![forbid(unsafe_code)]
//! Engine-neutral execution boundary. Memory binding and state codec are pending P1.

use core::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct EngineCaps {
    pub instruction_count: bool,
    pub precise_exceptions: bool,
    pub exclusive_monitor: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StopReason {
    BudgetExhausted,
    WaitingForInterrupt,
    Halted,
    Breakpoint,
    CapabilityMissing,
    Fault,
}

pub trait CpuEngine: Send {
    fn caps(&self) -> EngineCaps;
    fn run(&mut self, instructions: NonZeroU64) -> StopReason;
}
