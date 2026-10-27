//! `pdm` contract: PDM digital microphone clock/data pair.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PdmError {
    NoMic,
    ClockStopped,
    BadRate,
}

/// A packed PDM bitstream slice (1 bit per mic clock).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdmSamples {
    pub bits: Vec<u8>,
}

pub trait PdmSource {
    fn set_rate(&mut self, ctx: &mut dyn Ctx, hz: u32) -> Result<(), PdmError>;
    fn capture(&mut self, ctx: &mut dyn Ctx, count: usize) -> Result<PdmSamples, PdmError>;
}

pub trait PdmMic {
    fn on_rate(&mut self, ctx: &mut dyn Ctx, hz: u32) -> Result<(), PdmError>;
    fn sample(&mut self, ctx: &mut dyn Ctx, count: usize) -> Result<PdmSamples, PdmError>;
}
