//! `parallel` contract: IEEE 1284 parallel port (SPP/EPP/ECP modes).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ParallelError {
    NoTarget,
    BadMode,
    Busy,
    Timeout,
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelMode { Spp, Epp, Ecp }

pub trait ParallelInitiator {
    fn write_data(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), ParallelError>;
    fn read_data(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError>;
    fn write_control(&mut self, ctx: &mut dyn Ctx, ctrl: u8) -> Result<(), ParallelError>;
    fn read_status(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError>;
    fn epp_write(&mut self, ctx: &mut dyn Ctx, addr: u8, data: &[u8]) -> Result<(), ParallelError>;
    fn ecp_write(&mut self, ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), ParallelError>;
}

pub trait ParallelTarget {
    fn mode(&self) -> ParallelMode;
    fn write_data(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), ParallelError>;
    fn read_data(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError>;
    fn write_control(&mut self, ctx: &mut dyn Ctx, ctrl: u8) -> Result<(), ParallelError>;
    fn read_status(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError>;
}
