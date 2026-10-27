//! `spdif` contract: S/PDIF / IEC 60958 digital audio (consumer & professional).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SpdifError {
    NoReceiver,
    BadBiphase,
    Overflow,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SpdifFrame {
    /// 16-bit AES3 subframe: two 24-bit samples + 4-bit preamble + 4 VUC bits.
    pub sample_l: i32,
    pub sample_r: i32,
    pub validity: bool,
    pub user_data: bool,
    pub channel_status: bool,
    pub parity: bool,
}

pub trait SpdifTransmitter {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frames: &[SpdifFrame]) -> Result<(), SpdifError>;
}

pub trait SpdifReceiver {
    fn on_frames(&mut self, ctx: &mut dyn Ctx, frames: &[SpdifFrame]) -> Result<(), SpdifError>;
}
