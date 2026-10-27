//! `nand` contract: raw NAND flash bus (CLE/ALE/RE/WE cycles).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NandError {
    NoDevice,
    BadCommand,
    Timeout,
    EccUncorrectable,
    BadBlock,
}

/// One bus operation: command, address cycles, then write data in / read data out.
#[derive(Clone, Debug, Default)]
pub struct NandOp {
    pub command: u8,
    pub address: Vec<u8>,
    pub data: Vec<u8>,
}

pub trait NandHost {
    fn execute(&mut self, ctx: &mut dyn Ctx, op: &NandOp) -> Result<Vec<u8>, NandError>;
    fn ready(&mut self, ctx: &mut dyn Ctx) -> Result<bool, NandError>;
}

pub trait NandDevice {
    fn on_command(&mut self, ctx: &mut dyn Ctx, op: &NandOp) -> Result<Vec<u8>, NandError>;
    fn ready(&self) -> bool;
}
