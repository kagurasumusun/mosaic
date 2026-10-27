//! `espi` contract: Intel Enhanced Serial Peripheral Interface (eSPI).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum EspiError {
    NoTarget,
    BadCycle,
    Crc,
    NotResponding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspiCycle {
    Peripheral,
    VirtualWire,
    Oob,
    Flash,
    FlashUpp,
}

#[derive(Clone, Debug, Default)]
pub struct EspiPacket {
    pub cycle: u8,
    pub tag: u8,
    pub length: u32,
    pub payload: Vec<u8>,
}

pub trait EspiMaster {
    fn transfer(&mut self, ctx: &mut dyn Ctx, p: &EspiPacket) -> Result<Vec<u8>, EspiError>;
}

pub trait EspiTarget {
    fn on_transfer(&mut self, ctx: &mut dyn Ctx, p: &EspiPacket) -> Result<Vec<u8>, EspiError>;
}
