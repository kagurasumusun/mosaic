//! `scsi` bus entity (ADR 0025): SCSI bus (parallel): command/response framing.

use std::collections::BTreeMap;
use mosaic_contract::model::Ctx;
use mosaic_contract::scsi::{ScsiError, ScsiInitiator, ScsiTarget};

pub struct ScsiBus { targets: BTreeMap<u8, Box<dyn ScsiTarget>> }

impl Default for ScsiBus { fn default() -> Self { Self::new() } }

impl ScsiBus {
    pub fn new() -> Self { Self { targets: BTreeMap::new() } }
    pub fn add_target(&mut self, t: Box<dyn ScsiTarget>) { self.targets.insert(t.lun(), t); }
    fn tgt(&mut self, lun: u8) -> Result<&mut Box<dyn ScsiTarget>, ScsiError> {
        self.targets.get_mut(&lun).ok_or(ScsiError::NoTarget)
    }
}

impl ScsiInitiator for ScsiBus {
    fn command(&mut self, ctx: &mut dyn Ctx, lun: u8, cdb: &[u8]) -> Result<Vec<u8>, ScsiError> {
        self.tgt(lun)?.command(ctx, cdb)
    }
    fn request_sense(&mut self, ctx: &mut dyn Ctx, lun: u8) -> Result<Vec<u8>, ScsiError> {
        self.tgt(lun)?.sense(ctx)
    }
}
