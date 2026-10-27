//! `gpib` contract: IEEE-488 (GPIB) instrument bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GpibError {
    NoListener,
    NotTalker,
    Timeout,
    BusBusy,
}

pub trait GpibController {
    /// Interface message (ATN asserted), e.g. UNL 0x3F / UNT 0x5F / LLO 0x11.
    fn send_command(&mut self, ctx: &mut dyn Ctx, command: u8) -> Result<(), GpibError>;
    fn write(&mut self, ctx: &mut dyn Ctx, pad: u8, data: &[u8]) -> Result<(), GpibError>;
    fn read(&mut self, ctx: &mut dyn Ctx, pad: u8, budget: usize) -> Result<Vec<u8>, GpibError>;
}

pub trait GpibDevice {
    /// Primary address (0-30; 31 is never assigned).
    fn address(&self) -> u8;
    fn on_command(&mut self, ctx: &mut dyn Ctx, command: u8) -> Result<(), GpibError>;
    fn on_write(&mut self, ctx: &mut dyn Ctx, data: &[u8]) -> Result<(), GpibError>;
    fn read(&mut self, ctx: &mut dyn Ctx, budget: usize) -> Result<Vec<u8>, GpibError>;
}
