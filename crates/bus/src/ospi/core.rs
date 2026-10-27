//! `ospi` bus entity (ADR 0025): Octal SPI: 8-data-line extension of QSPI.

use mosaic_contract::model::Ctx;
use mosaic_contract::ospi::{OspiError, OspiInitiator, OspiTarget, OspiWidth};

pub struct OspiBus {
    target: Option<Box<dyn OspiTarget>>,
}

impl Default for OspiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl OspiBus {
    pub fn new() -> Self {
        Self { target: None }
    }
    pub fn attach(&mut self, t: Box<dyn OspiTarget>) {
        self.target = Some(t);
    }
}

impl OspiInitiator for OspiBus {
    fn command(
        &mut self,
        ctx: &mut dyn Ctx,
        cmd: u32,
        addr: Option<u32>,
        width: OspiWidth,
    ) -> Result<(), OspiError> {
        self.target
            .as_mut()
            .ok_or(OspiError::NoTarget)?
            .command(ctx, cmd, addr, width)
    }
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        width: OspiWidth,
        len: usize,
    ) -> Result<Vec<u8>, OspiError> {
        self.target
            .as_mut()
            .ok_or(OspiError::NoTarget)?
            .read(ctx, width, len)
    }
    fn write(&mut self, ctx: &mut dyn Ctx, width: OspiWidth, data: &[u8]) -> Result<(), OspiError> {
        self.target
            .as_mut()
            .ok_or(OspiError::NoTarget)?
            .write(ctx, width, data)
    }
}
