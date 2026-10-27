//! `ospi` contract: Octal SPI: 8-data-line extension of QSPI.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OspiError {
    NoTarget,
    BadMode,
    Overflow,
    Truncated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OspiWidth { Single, Dual, Quad, Octal }

pub trait OspiInitiator {
    fn command(&mut self, ctx: &mut dyn Ctx, cmd: u32, addr: Option<u32>, width: OspiWidth) -> Result<(), OspiError>;
    fn read(&mut self, ctx: &mut dyn Ctx, width: OspiWidth, len: usize) -> Result<Vec<u8>, OspiError>;
    fn write(&mut self, ctx: &mut dyn Ctx, width: OspiWidth, data: &[u8]) -> Result<(), OspiError>;
}

pub trait OspiTarget {
    fn command(&mut self, ctx: &mut dyn Ctx, cmd: u32, addr: Option<u32>, width: OspiWidth) -> Result<(), OspiError>;
    fn read(&mut self, ctx: &mut dyn Ctx, width: OspiWidth, len: usize) -> Result<Vec<u8>, OspiError>;
    fn write(&mut self, ctx: &mut dyn Ctx, width: OspiWidth, data: &[u8]) -> Result<(), OspiError>;
}
