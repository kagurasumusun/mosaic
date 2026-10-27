//! `parallel` bus entity (ADR 0025): IEEE 1284 parallel port (SPP/EPP/ECP modes).

use mosaic_contract::model::Ctx;
use mosaic_contract::parallel::{ParallelError, ParallelInitiator, ParallelMode, ParallelTarget};

pub struct ParallelBus { target: Option<Box<dyn ParallelTarget>> }

impl Default for ParallelBus { fn default() -> Self { Self::new() } }

impl ParallelBus {
    pub fn new() -> Self { Self { target: None } }
    pub fn attach(&mut self, t: Box<dyn ParallelTarget>) { self.target = Some(t); }
}

impl ParallelInitiator for ParallelBus {
    fn write_data(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), ParallelError> {
        self.target.as_mut().ok_or(ParallelError::NoTarget)?.write_data(ctx, byte)
    }
    fn read_data(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError> {
        self.target.as_mut().ok_or(ParallelError::NoTarget)?.read_data(ctx)
    }
    fn write_control(&mut self, ctx: &mut dyn Ctx, ctrl: u8) -> Result<(), ParallelError> {
        self.target.as_mut().ok_or(ParallelError::NoTarget)?.write_control(ctx, ctrl)
    }
    fn read_status(&mut self, ctx: &mut dyn Ctx) -> Result<u8, ParallelError> {
        self.target.as_mut().ok_or(ParallelError::NoTarget)?.read_status(ctx)
    }
    fn epp_write(&mut self, ctx: &mut dyn Ctx, _addr: u8, data: &[u8]) -> Result<(), ParallelError> {
        for &b in data { self.write_data(ctx, b)?; }
        Ok(())
    }
    fn ecp_write(&mut self, ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), ParallelError> {
        for &b in data { self.write_data(ctx, b)?; }
        Ok(())
    }
}
