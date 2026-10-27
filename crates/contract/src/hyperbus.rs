//! `hyperbus` contract: HyperFlash/HyperRAM 16-bit DDR bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HyperbusError {
    NoDevice,
    BadCommand,
    Timeout,
    WriteProtected,
}

pub trait HyperbusHost {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: usize,
    ) -> Result<Vec<u16>, HyperbusError>;
    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: &[u16],
    ) -> Result<(), HyperbusError>;
    fn register_read(&mut self, ctx: &mut dyn Ctx, register: u16) -> Result<u16, HyperbusError>;
    fn register_write(
        &mut self,
        ctx: &mut dyn Ctx,
        register: u16,
        value: u16,
    ) -> Result<(), HyperbusError>;
}

pub trait HyperbusDevice {
    fn on_read(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: usize,
    ) -> Result<Vec<u16>, HyperbusError>;
    fn on_write(
        &mut self,
        ctx: &mut dyn Ctx,
        address: u64,
        words: &[u16],
    ) -> Result<(), HyperbusError>;
    fn on_register_read(&mut self, ctx: &mut dyn Ctx, register: u16) -> Result<u16, HyperbusError>;
    fn on_register_write(
        &mut self,
        ctx: &mut dyn Ctx,
        register: u16,
        value: u16,
    ) -> Result<(), HyperbusError>;
}
