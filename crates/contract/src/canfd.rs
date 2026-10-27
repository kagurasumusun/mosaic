//! `canfd` contract: CAN FD (Flexible Data-rate): extended from CAN with up to 64-byte frames.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CanFdError {
    NoAck,
    BitError,
    StuffError,
    CRCError,
    FormError,
    Overflow,
    BusOff,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CanFdMode {
    Classic,
    Fd,
    FdBrs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanFdFrame {
    pub id: u32,
    pub extended: bool,
    pub fd: bool,
    pub brs: bool,
    pub esi: bool,
    pub dlc: u8,
    pub data: Vec<u8>,
}

pub trait CanFdInitiator {
    fn transmit(&mut self, ctx: &mut dyn Ctx, frame: &CanFdFrame) -> Result<(), CanFdError>;
    fn receive(&mut self, ctx: &mut dyn Ctx) -> Result<CanFdFrame, CanFdError>;
}

pub trait CanFdTarget {
    fn accepts(&self, frame: &CanFdFrame) -> bool;
    fn receive(&mut self, ctx: &mut dyn Ctx, frame: &CanFdFrame) -> Result<(), CanFdError>;
    fn transmit(&mut self, ctx: &mut dyn Ctx) -> Result<Option<CanFdFrame>, CanFdError>;
}
