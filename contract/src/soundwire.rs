//! `soundwire` contract: MIPI SoundWire audio bus.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SoundWireError {
    NoPeripheral,
    BadParity,
    Collision,
    UnsupportedRow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SwRow {
    Control,
    Data(u8),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SoundWireFrame {
    /// 16 payload rows (control + data), 8 channels * 2 rows.
    pub rows: [u16; 16],
}

pub trait SoundWireMaster {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frame: &SoundWireFrame) -> Result<(), SoundWireError>;
    fn receive(&mut self, ctx: &mut dyn Ctx) -> Result<SoundWireFrame, SoundWireError>;
}

pub trait SoundWirePeripheral {
    fn on_frame(&mut self, ctx: &mut dyn Ctx, frame: &SoundWireFrame) -> Result<(), SoundWireError>;
    fn reply(&mut self, ctx: &mut dyn Ctx, frame: &mut SoundWireFrame) -> Result<(), SoundWireError>;
}
