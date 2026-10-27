//! `displayport` contract: VESA DisplayPort main link + AUX channel.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DisplayPortError {
    NoSink,
    HpdDeasserted,
    AuxTimeout,
    AuxNack,
    BadFrame,
}

#[derive(Clone, Debug, Default)]
pub struct DpVideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub trait DisplayPortSource {
    fn push_video(&mut self, ctx: &mut dyn Ctx, frame: &DpVideoFrame)
        -> Result<(), DisplayPortError>;
    fn aux_read(&mut self, ctx: &mut dyn Ctx, address: u32) -> Result<Vec<u8>, DisplayPortError>;
    fn aux_write(&mut self, ctx: &mut dyn Ctx, address: u32, data: &[u8])
        -> Result<(), DisplayPortError>;
    fn read_dpcd(&mut self, ctx: &mut dyn Ctx, address: u32) -> Result<u32, DisplayPortError>;
}

pub trait DisplayPortSink {
    fn on_video(&mut self, ctx: &mut dyn Ctx, frame: &DpVideoFrame) -> Result<(), DisplayPortError>;
    fn hpd(&self) -> bool;
    fn aux_handle(&mut self, ctx: &mut dyn Ctx, write: bool, address: u32, data: &mut Vec<u8>)
        -> Result<(), DisplayPortError>;
}
