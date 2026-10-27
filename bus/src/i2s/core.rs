//! `i2s` bus entity (ADR 0025): I2S/PCM audio serial bus (Philips/TDM modes).

use mosaic_contract::i2s::{I2sError, I2sInitiator, I2sTarget};
use mosaic_contract::model::Ctx;

pub struct I2sBus { targets: Vec<Box<dyn I2sTarget>> }

impl Default for I2sBus { fn default() -> Self { Self::new() } }

impl I2sBus {
    pub fn new() -> Self { Self { targets: Vec::new() } }
    pub fn add_target(&mut self, t: Box<dyn I2sTarget>) { self.targets.push(t); }
}

impl I2sInitiator for I2sBus {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frames: &[u32]) -> Result<(), I2sError> {
        for t in self.targets.iter_mut() { t.transmit(ctx, frames)?; }
        Ok(())
    }
    fn receive(&mut self, ctx: &mut dyn Ctx, frames: usize) -> Result<Vec<u32>, I2sError> {
        if self.targets.is_empty() { return Err(I2sError::NoTarget); }
        self.targets[0].receive(ctx, frames)
    }
}
