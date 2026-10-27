//! `pcmcia` contract: 16-bit PC Card bus (attribute/CIS + memory + I/O).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PcmciaError {
    NoCard,
    BadAttribute,
    IoFault,
    VoltageMismatch,
}

pub trait PcmciaHost {
    fn attribute_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u16, PcmciaError>;
    fn attribute_write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u32,
        value: u16,
    ) -> Result<(), PcmciaError>;
    fn io_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u8, PcmciaError>;
    fn io_write(&mut self, ctx: &mut dyn Ctx, offset: u32, value: u8) -> Result<(), PcmciaError>;
}

pub trait PcmciaCard {
    /// Attribute-space (CIS) read: odd/even tuple bytes.
    fn attribute_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u16, PcmciaError>;
    fn attribute_write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u32,
        value: u16,
    ) -> Result<(), PcmciaError>;
    fn io_read(&mut self, ctx: &mut dyn Ctx, offset: u32) -> Result<u8, PcmciaError>;
    fn io_write(&mut self, ctx: &mut dyn Ctx, offset: u32, value: u8) -> Result<(), PcmciaError>;
}
