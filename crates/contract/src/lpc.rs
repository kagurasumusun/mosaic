//! `lpc` contract: Intel Low Pin Count bus (LPC): firmware, super-IO, TPM.

use crate::model::Ctx;

/// LPC error modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LpcError {
    NoTarget,
    BadCycle,
    Sync,
    NoAddress,
    Truncated,
}

/// LPC cycle types.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LpcCycle {
    IoRead(u16),
    IoWrite(u16, u8),
    MemRead(u32),
    MemWrite(u32, u8),
    FirmwareRead(u32),
    FirmwareWrite(u32, u8),
}

/// Initiator port: the host CPU drives LPC cycles.
pub trait LpcInitiator {
    fn cycle(&mut self, ctx: &mut dyn Ctx, cycle: LpcCycle) -> Result<u8, LpcError>;
}

/// Target port: a peripheral responds to LPC.
pub trait LpcTarget {
    fn accepts(&self, cycle: &LpcCycle) -> bool;
    fn cycle(&mut self, ctx: &mut dyn Ctx, cycle: LpcCycle) -> Result<u8, LpcError>;
}
