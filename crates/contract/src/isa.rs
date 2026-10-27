//! `isa` contract: classic 8/16-bit ISA bus (I/O port cycles + IRQ lines).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum IsaError {
    NoCard,
    IoFault,
    BadWidth,
    BadDmaChannel,
}

pub trait IsaHost {
    fn io_read(&mut self, ctx: &mut dyn Ctx, port: u16, width: u8) -> Result<u64, IsaError>;
    fn io_write(
        &mut self,
        ctx: &mut dyn Ctx,
        port: u16,
        width: u8,
        value: u64,
    ) -> Result<(), IsaError>;
}

pub trait IsaCard {
    /// Does this card decode the port? Only a claiming card sees the cycle.
    fn claim_io(&self, port: u16) -> bool;
    fn io_read(&mut self, ctx: &mut dyn Ctx, port: u16, width: u8) -> Result<u64, IsaError>;
    fn io_write(
        &mut self,
        ctx: &mut dyn Ctx,
        port: u16,
        width: u8,
        value: u64,
    ) -> Result<(), IsaError>;
}
