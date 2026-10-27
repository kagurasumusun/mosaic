//! `dvi` contract: DVI digital video link (single/dual-link TMDS + DDC EDID).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DviError {
    NoSink,
    BadFrame,
    HpdDeasserted,
    EdidReadFail,
    LinkDown,
}

/// One frame of TMDS video, packed 24 bpp RGB.
#[derive(Clone, Debug, Default)]
pub struct DviFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub trait DviSource {
    fn push_video(&mut self, ctx: &mut dyn Ctx, frame: &DviFrame) -> Result<(), DviError>;
    fn read_edid(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, DviError>;
}

pub trait DviSink {
    fn on_video(&mut self, ctx: &mut dyn Ctx, frame: &DviFrame) -> Result<(), DviError>;
    fn edid(&self) -> &[u8];
    fn hpd(&self) -> bool;
}
