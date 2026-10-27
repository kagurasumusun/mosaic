//! `lpc` bus entity (ADR 0025): Intel Low Pin Count bus (LPC): firmware, super-IO, TPM.

use mosaic_contract::lpc::{LpcCycle, LpcError, LpcInitiator, LpcTarget};
use mosaic_contract::model::Ctx;

/// LPC bus entity: routes cycles to the first target that accepts them.
pub struct LpcBus { targets: Vec<Box<dyn LpcTarget>> }

impl Default for LpcBus { fn default() -> Self { Self::new() } }

impl LpcBus {
    pub fn new() -> Self { Self { targets: Vec::new() } }
    pub fn add_target(&mut self, t: Box<dyn LpcTarget>) { self.targets.push(t); }
}

impl LpcInitiator for LpcBus {
    fn cycle(&mut self, ctx: &mut dyn Ctx, cycle: LpcCycle) -> Result<u8, LpcError> {
        for t in self.targets.iter_mut() {
            if t.accepts(&cycle) { return t.cycle(ctx, cycle); }
        }
        Err(LpcError::NoTarget)
    }
}
