//! `qspi` bus entity (ADR 0025): Quad SPI: 4-data-line SPI extension (also basis for OSPI).

use mosaic_contract::model::Ctx;
use mosaic_contract::qspi::{QspiError, QspiInitiator, QspiTarget, QspiWidth};

pub struct QspiBus {
    target: Option<Box<dyn QspiTarget>>,
}

impl Default for QspiBus {
    fn default() -> Self {
        Self::new()
    }
}

impl QspiBus {
    pub fn new() -> Self {
        Self { target: None }
    }
    pub fn attach(&mut self, t: Box<dyn QspiTarget>) {
        self.target = Some(t);
    }
}

impl QspiInitiator for QspiBus {
    fn command(
        &mut self,
        ctx: &mut dyn Ctx,
        cmd: u8,
        addr: Option<u32>,
        width: QspiWidth,
    ) -> Result<(), QspiError> {
        self.target
            .as_mut()
            .ok_or(QspiError::NoTarget)?
            .command(ctx, cmd, addr, width)
    }
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        width: QspiWidth,
        len: usize,
    ) -> Result<Vec<u8>, QspiError> {
        self.target
            .as_mut()
            .ok_or(QspiError::NoTarget)?
            .read(ctx, width, len)
    }
    fn write(&mut self, ctx: &mut dyn Ctx, width: QspiWidth, data: &[u8]) -> Result<(), QspiError> {
        self.target
            .as_mut()
            .ok_or(QspiError::NoTarget)?
            .write(ctx, width, data)
    }
}
