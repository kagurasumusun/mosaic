//! `microwire` bus entity (ADR 0025): Microwire (National Semiconductor): 3-wire serial EEPROM bus.

use mosaic_contract::microwire::{MicrowireError, MicrowireInitiator, MicrowireOp, MicrowireTarget};
use mosaic_contract::model::Ctx;

pub struct MicrowireBus { target: Option<Box<dyn MicrowireTarget>> }

impl Default for MicrowireBus { fn default() -> Self { Self::new() } }

impl MicrowireBus {
    pub fn new() -> Self { Self { target: None } }
    pub fn attach(&mut self, t: Box<dyn MicrowireTarget>) { self.target = Some(t); }
}

impl MicrowireInitiator for MicrowireBus {
    fn send(&mut self, ctx: &mut dyn Ctx, op: MicrowireOp, addr: u16, data: &[u8]) -> Result<Vec<u8>, MicrowireError> {
        self.target.as_mut().ok_or(MicrowireError::NoTarget)?.execute(ctx, op, addr, data)
    }
}
