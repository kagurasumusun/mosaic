//! `hdmi` contract: HDMI digital video/audio link (TMDS + DDC + CEC).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum HdmiError {
    NoSink,
    BadFrame,
    HpdDeasserted,
    EdidReadFail,
    CecNack,
}

/// 8-channel IEC 60958 audio sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioSample { pub channels: [u32; 8] }

/// A vertical-refresh frame handed to the sink.
#[derive(Clone, Debug, Default)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Packed 24 bpp (8bpc RGB).
    pub pixels: Vec<u8>,
}

pub trait HdmiSource {
    fn push_video(&mut self, ctx: &mut dyn Ctx, frame: &VideoFrame) -> Result<(), HdmiError>;
    fn push_audio(&mut self, ctx: &mut dyn Ctx, sample: &AudioSample) -> Result<(), HdmiError>;
    fn read_edid(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, HdmiError>;
    fn cec_send(&mut self, ctx: &mut dyn Ctx, initiator: u8, destination: u8, payload: &[u8])
        -> Result<(), HdmiError>;
}

pub trait HdmiSink {
    fn on_video(&mut self, ctx: &mut dyn Ctx, frame: &VideoFrame) -> Result<(), HdmiError>;
    fn on_audio(&mut self, ctx: &mut dyn Ctx, sample: &AudioSample) -> Result<(), HdmiError>;
    fn edid(&self) -> &[u8];
    fn hpd(&self) -> bool;
    fn cec_receive(&mut self, ctx: &mut dyn Ctx, initiator: u8, destination: u8,
                   payload: &[u8]) -> Result<(), HdmiError>;
}
