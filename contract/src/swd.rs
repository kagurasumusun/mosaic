//! `swd` contract: ARM Serial Wire Debug (SW-DP): DP/AP register access.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SwdError {
    NoAck,
    Fault,
    Wait,
    Parity,
    BadAp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwdAp { Dp, Ap(u8) }

pub trait SwdInitiator {
    fn read(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8) -> Result<u32, SwdError>;
    fn write(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8, value: u32) -> Result<(), SwdError>;
}

pub trait SwdTarget {
    fn read(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8) -> Result<u32, SwdError>;
    fn write(&mut self, ctx: &mut dyn Ctx, ap: SwdAp, addr: u8, value: u32) -> Result<(), SwdError>;
}
