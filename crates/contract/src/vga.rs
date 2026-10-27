//! `vga` contract: analog RGB video link (VGA connector + DDC EDID).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum VgaError {
    NoSink,
    BadMode,
    DdcNack,
    HsyncOutOfRange,
}

/// An analog RGB display mode (VESA GTF/CVT timing family).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VgaMode {
    pub width: u32,
    pub height: u32,
    /// Field rate in Hz.
    pub hz: u32,
    /// Pixel clock in kHz.
    pub pixel_clock_khz: u32,
}

/// One frame of analog video, sampled to packed 24 bpp RGB.
#[derive(Clone, Debug, Default)]
pub struct VgaFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub trait VgaSource {
    fn set_mode(&mut self, ctx: &mut dyn Ctx, mode: &VgaMode) -> Result<(), VgaError>;
    fn push_frame(&mut self, ctx: &mut dyn Ctx, frame: &VgaFrame) -> Result<(), VgaError>;
    fn read_edid(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, VgaError>;
}

pub trait VgaSink {
    fn on_mode(&mut self, ctx: &mut dyn Ctx, mode: &VgaMode) -> Result<(), VgaError>;
    fn on_frame(&mut self, ctx: &mut dyn Ctx, frame: &VgaFrame) -> Result<(), VgaError>;
    fn edid(&self) -> &[u8];
}
