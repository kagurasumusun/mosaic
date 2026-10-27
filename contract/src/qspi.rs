//! `qspi` contract: Quad SPI: 4-data-line SPI extension (also basis for OSPI).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum QspiError {
    NoTarget,
    BadMode,
    Overflow,
    Truncated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QspiWidth { Single, Dual, Quad }

pub trait QspiInitiator {
    fn command(&mut self, ctx: &mut dyn Ctx, cmd: u8, addr: Option<u32>, width: QspiWidth) -> Result<(), QspiError>;
    fn read(&mut self, ctx: &mut dyn Ctx, width: QspiWidth, len: usize) -> Result<Vec<u8>, QspiError>;
    fn write(&mut self, ctx: &mut dyn Ctx, width: QspiWidth, data: &[u8]) -> Result<(), QspiError>;
}

pub trait QspiTarget {
    fn command(&mut self, ctx: &mut dyn Ctx, cmd: u8, addr: Option<u32>, width: QspiWidth) -> Result<(), QspiError>;
    fn read(&mut self, ctx: &mut dyn Ctx, width: QspiWidth, len: usize) -> Result<Vec<u8>, QspiError>;
    fn write(&mut self, ctx: &mut dyn Ctx, width: QspiWidth, data: &[u8]) -> Result<(), QspiError>;
}
