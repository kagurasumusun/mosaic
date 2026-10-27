//! `mipi_csi` contract: MIPI CSI-2 (Camera Serial Interface).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CsiError {
    NoSource,
    NoReceiver,
    BadPacket,
    CrcMismatch,
    Timeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CsiDataType {
    Yuv422_8,
    Raw8,
    Raw10,
    Raw12,
    Raw14,
    Rgb565,
    Rgb888,
    Embedded,
    GenericShort(u8),
    GenericLong(u8),
}

#[derive(Clone, Debug, Default)]
pub struct CsiPacket {
    pub vc: u8,
    pub dt: u8,
    pub payload: Vec<u8>,
}

pub trait CsiReceiver {
    fn on_packet(&mut self, ctx: &mut dyn Ctx, p: &CsiPacket) -> Result<(), CsiError>;
}

pub trait CsiSource {
    fn emit(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<CsiPacket>, CsiError>;
}
