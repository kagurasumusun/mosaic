#![no_std]
#![forbid(unsafe_code)]
//! Engine-neutral, synchronous single-core execution. No vendor types escape.

use core::num::NonZeroU64;
use mosaic_core_types::{Addr, Permissions};

pub use mosaic_core_types::RegId;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct EngineCaps {
    pub instruction_count: bool,
    pub precise_exceptions: bool,
    pub exclusive_monitor: bool,
}

impl EngineCaps {
    /// Functional instruction budgets; not precise exception or SMP guarantees.
    pub const COUNTED: Self = Self {
        instruction_count: true,
        precise_exceptions: false,
        exclusive_monitor: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EngineError {
    InvalidRegister,
    InvalidValue,
    InvalidMapping,
    Overlap,
    Unmapped,
    Backend,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunOutcome {
    pub reason: StopReason,
    /// Instruction hooks observed. On Fault this is not a retired-instruction
    /// count: the last instruction may have failed after its pre-execution hook.
    pub instruction_hooks: u64,
}

pub trait CpuEngine: Send {
    fn caps(&self) -> EngineCaps;
    fn run(&mut self, instructions: NonZeroU64) -> RunOutcome;
    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError>;
    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError>;
    /// Initial private-memory path. A future AddressView/DMI binding replaces
    /// this without exposing raw backend handles or host pointers to models.
    fn map_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
    ) -> Result<(), EngineError>;
    /// Loader/debug access, not guest access: writes may initialize ROM and code
    /// pages. Patching executable bytes must invalidate translated code.
    fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError>;
    fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError>;
}
