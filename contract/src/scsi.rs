//! `scsi` contract: SCSI bus (parallel): command/response framing.

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ScsiError {
    NoTarget,
    BadLun,
    CheckCondition,
    Busy,
    Reservation,
    Overflow,
}

pub trait ScsiInitiator {
    fn command(&mut self, ctx: &mut dyn Ctx, lun: u8, cdb: &[u8]) -> Result<Vec<u8>, ScsiError>;
    fn request_sense(&mut self, ctx: &mut dyn Ctx, lun: u8) -> Result<Vec<u8>, ScsiError>;
}

pub trait ScsiTarget {
    fn lun(&self) -> u8;
    fn command(&mut self, ctx: &mut dyn Ctx, cdb: &[u8]) -> Result<Vec<u8>, ScsiError>;
    fn sense(&mut self, ctx: &mut dyn Ctx) -> Result<Vec<u8>, ScsiError>;
}
