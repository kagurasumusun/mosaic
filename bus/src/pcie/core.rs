//! `pcie` bus entity (ADR 0025): PCI Express: TLP routing (distinct from legacy PCI config space).

use std::collections::BTreeMap;
use mosaic_contract::model::Ctx;
use mosaic_contract::pcie::{PcieError, PcieInitiator, PcieTarget, TlpType};

pub struct PcieBus { devices: BTreeMap<u16, Box<dyn PcieTarget>> }

impl Default for PcieBus { fn default() -> Self { Self::new() } }

impl PcieBus {
    pub fn new() -> Self { Self { devices: BTreeMap::new() } }
    pub fn attach(&mut self, t: Box<dyn PcieTarget>) { self.devices.insert(t.bdf(), t); }
}

impl PcieInitiator for PcieBus {
    fn route(&mut self, ctx: &mut dyn Ctx, bdf: u16, tlp: TlpType, addr: u64, data: &[u8]) -> Result<Vec<u8>, PcieError> {
        self.devices.get_mut(&bdf).ok_or(PcieError::NoDevice)?.route(ctx, tlp, addr, data)
    }
}
