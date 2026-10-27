//! `lin` contract: LIN (Local Interconnect Network): single-master framed bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LinError {
    NoResponder,
    BadId,
    Checksum,
    Parity,
    Overflow,
}

pub trait LinInitiator {
    fn send_header(&mut self, ctx: &mut dyn Ctx, id: u8) -> Result<(), LinError>;
    fn exchange(&mut self, ctx: &mut dyn Ctx, id: u8, write: &mut [u8]) -> Result<usize, LinError>;
}

pub trait LinTarget {
    fn pid(&self) -> u8;
    fn exchange(&mut self, ctx: &mut dyn Ctx, id: u8, write: &mut [u8]) -> Result<usize, LinError>;
}
