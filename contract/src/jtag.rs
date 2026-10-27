//! `jtag` contract: JTAG TAP (IEEE 1149.1): test access port, IR/DR scan.

use crate::model::Ctx;

/// JTAG error modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum JtagError {
    NoTdo,
    BadState,
    Overflow,
}

/// JTAG TAP controller state (simplified).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JtagState {
    TestLogicReset,
    RunTestIdle,
    SelectDrScan,
    CaptureDr,
    ShiftDr,
    Exit1Dr,
    PauseDr,
    Exit2Dr,
    UpdateDr,
    SelectIrScan,
    CaptureIr,
    ShiftIr,
    Exit1Ir,
    PauseIr,
    Exit2Ir,
    UpdateIr,
}

/// Initiator port: drives TCK/TMS/TDI and observes TDO.
pub trait JtagInitiator {
    fn shift_ir(&mut self, ctx: &mut dyn Ctx, tdi: &[u8]) -> Result<Vec<u8>, JtagError>;
    fn shift_dr(&mut self, ctx: &mut dyn Ctx, tdi: &[u8]) -> Result<Vec<u8>, JtagError>;
    fn pulse_tck(&mut self, ctx: &mut dyn Ctx, tms: bool, tdi: bool) -> Result<bool, JtagError>;
    fn state(&self) -> JtagState;
}

/// Target port: a TAP responds with TDO bits.
pub trait JtagTarget {
    fn tdo(&mut self, ctx: &mut dyn Ctx, tdi: &[u8], ir: bool) -> Result<Vec<u8>, JtagError>;
    fn current_ir(&self) -> u32;
}
