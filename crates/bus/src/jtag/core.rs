//! `jtag` bus entity (ADR 0025): JTAG TAP (IEEE 1149.1): test access port, IR/DR scan.

use mosaic_contract::jtag::{JtagError, JtagInitiator, JtagState, JtagTarget};
use mosaic_contract::model::Ctx;

/// JTAG chain entity: a single TAP or a daisy chain.
pub struct JtagChain {
    taps: Vec<Box<dyn JtagTarget>>,
    state: JtagState,
    ir: u32,
}

impl Default for JtagChain {
    fn default() -> Self {
        Self::new()
    }
}

impl JtagChain {
    pub fn new() -> Self {
        Self {
            taps: Vec::new(),
            state: JtagState::TestLogicReset,
            ir: 0,
        }
    }
    pub fn add_tap(&mut self, t: Box<dyn JtagTarget>) {
        self.taps.push(t);
    }
}

impl JtagInitiator for JtagChain {
    fn shift_ir(&mut self, ctx: &mut dyn Ctx, tdi: &[u8]) -> Result<Vec<u8>, JtagError> {
        if self.taps.is_empty() {
            return Err(JtagError::NoTdo);
        }
        let out = self.taps[0].tdo(ctx, tdi, true)?;
        self.ir = self.taps[0].current_ir();
        self.state = JtagState::ShiftIr;
        Ok(out)
    }
    fn shift_dr(&mut self, ctx: &mut dyn Ctx, tdi: &[u8]) -> Result<Vec<u8>, JtagError> {
        if self.taps.is_empty() {
            return Err(JtagError::NoTdo);
        }
        let mut out = Vec::new();
        for tap in self.taps.iter_mut() {
            out.extend_from_slice(&tap.tdo(ctx, tdi, false)?);
        }
        self.state = JtagState::ShiftDr;
        Ok(out)
    }
    fn pulse_tck(&mut self, _ctx: &mut dyn Ctx, tms: bool, _tdi: bool) -> Result<bool, JtagError> {
        if tms {
            self.state = match self.state {
                JtagState::TestLogicReset | JtagState::RunTestIdle => JtagState::SelectDrScan,
                _ => JtagState::TestLogicReset,
            };
        }
        Ok(false)
    }
    fn state(&self) -> JtagState {
        self.state
    }
}
