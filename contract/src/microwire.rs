//! `microwire` contract: Microwire (National Semiconductor): 3-wire serial EEPROM bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MicrowireError {
    NoTarget,
    BadOp,
    Overflow,
    Truncated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MicrowireOp { Read, Write, Erase, Eral, Wral, Ewen, Ewds }

pub trait MicrowireInitiator {
    fn send(&mut self, ctx: &mut dyn Ctx, op: MicrowireOp, addr: u16, data: &[u8]) -> Result<Vec<u8>, MicrowireError>;
}

pub trait MicrowireTarget {
    fn instruction_bits(&self) -> u8;
    fn address_bits(&self) -> u8;
    fn execute(&mut self, ctx: &mut dyn Ctx, op: MicrowireOp, addr: u16, data: &[u8]) -> Result<Vec<u8>, MicrowireError>;
}
