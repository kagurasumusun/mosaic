//! `i2s` contract: I2S/PCM audio serial bus (Philips/TDM modes).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum I2sError {
    NoTarget,
    Overflow,
    Underrun,
    BadMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2sMode { Philips, LeftJustified, RightJustified, PcmLong, PcmShort }

pub trait I2sInitiator {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frames: &[u32]) -> Result<(), I2sError>;
    fn receive(&mut self, ctx: &mut dyn Ctx, frames: usize) -> Result<Vec<u32>, I2sError>;
}

pub trait I2sTarget {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frames: &[u32]) -> Result<(), I2sError>;
    fn receive(&mut self, ctx: &mut dyn Ctx, frames: usize) -> Result<Vec<u32>, I2sError>;
}
