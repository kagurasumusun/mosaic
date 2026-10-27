//! `mipi_dsi` contract: MIPI DSI (Display Serial Interface).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DsiError {
    NoPeripheral,
    BadCommand,
    Timeout,
    CrcMismatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DsiMode { Video, Command, Burst }

#[derive(Clone, Debug, Default)]
pub struct DsiPacket {
    pub vc: u8,
    pub data_type: u8,
    pub payload: Vec<u8>,
}

pub trait DsiHost {
    fn send_packet(&mut self, ctx: &mut dyn Ctx, p: &DsiPacket) -> Result<(), DsiError>;
    fn read_response(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, DsiError>;
    fn mode(&self) -> DsiMode;
}

pub trait DsiPeripheral {
    fn on_packet(&mut self, ctx: &mut dyn Ctx, p: &DsiPacket) -> Result<(), DsiError>;
    fn read_request(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, DsiError>;
}
