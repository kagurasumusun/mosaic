//! `lvds` contract: LVDS serializer/deserializer LCD panel link.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LvdsError {
    NoPanel,
    BadTiming,
    BacklightOff,
}

/// One frame of LVDS video, packed 24 bpp RGB.
#[derive(Clone, Debug, Default)]
pub struct LvdsFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub trait LvdsSource {
    fn set_backlight(&mut self, ctx: &mut dyn Ctx, level: u8) -> Result<(), LvdsError>;
    fn push_frame(&mut self, ctx: &mut dyn Ctx, frame: &LvdsFrame) -> Result<(), LvdsError>;
}

pub trait LvdsSink {
    fn on_backlight(&mut self, ctx: &mut dyn Ctx, level: u8) -> Result<(), LvdsError>;
    fn on_frame(&mut self, ctx: &mut dyn Ctx, frame: &LvdsFrame) -> Result<(), LvdsError>;
    fn backlight(&self) -> u8;
}
