//! `sata` bus entity (ADR 0025): SATA/AHCI bus: FIS-based frame delivery.

use mosaic_contract::model::Ctx;
use mosaic_contract::sata::{SataError, SataInitiator, SataTarget};

pub struct SataBus {
    target: Option<Box<dyn SataTarget>>,
}

impl Default for SataBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SataBus {
    pub fn new() -> Self {
        Self { target: None }
    }
    pub fn attach(&mut self, t: Box<dyn SataTarget>) {
        self.target = Some(t);
    }
}

impl SataInitiator for SataBus {
    fn send_fis(&mut self, ctx: &mut dyn Ctx, fis: &[u8]) -> Result<(), SataError> {
        self.target
            .as_mut()
            .ok_or(SataError::NoDevice)?
            .send_fis(ctx, fis)
    }
    fn recv_fis(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, SataError> {
        self.target
            .as_mut()
            .ok_or(SataError::NoDevice)?
            .recv_fis(ctx)
    }
}
