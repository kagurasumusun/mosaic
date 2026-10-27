//! `sata` contract: SATA/AHCI bus: FIS-based frame delivery.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SataError {
    NoDevice,
    BadFis,
    Overflow,
    Busy,
    LinkDown,
}

pub trait SataInitiator {
    fn send_fis(&mut self, ctx: &mut dyn Ctx, fis: &[u8]) -> Result<(), SataError>;
    fn recv_fis(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, SataError>;
}

pub trait SataTarget {
    fn send_fis(&mut self, ctx: &mut dyn Ctx, fis: &[u8]) -> Result<(), SataError>;
    fn recv_fis(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, SataError>;
}
