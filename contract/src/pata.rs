//! `pata` contract: PATA/IDE parallel bus (ATA/ATAPI command block).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PataError {
    NoDevice,
    BadReg,
    Aborted,
    Busy,
    Drq,
}

pub trait PataInitiator {
    fn write_reg(&mut self, ctx: &mut dyn Ctx, reg: u8, value: u8) -> Result<(), PataError>;
    fn read_reg(&mut self, ctx: &mut dyn Ctx, reg: u8) -> Result<u8, PataError>;
    fn pio_data(&mut self, ctx: &mut dyn Ctx, dev: u8, write: bool, buf: &mut [u16]) -> Result<usize, PataError>;
}

pub trait PataTarget {
    fn selected(&self, dev: u8) -> bool;
    fn write_reg(&mut self, ctx: &mut dyn Ctx, reg: u8, value: u8) -> Result<(), PataError>;
    fn read_reg(&mut self, ctx: &mut dyn Ctx, reg: u8) -> Result<u8, PataError>;
    fn pio_data(&mut self, ctx: &mut dyn Ctx, write: bool, buf: &mut [u16]) -> Result<usize, PataError>;
}
