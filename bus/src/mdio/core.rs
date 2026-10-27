//! `mdio` bus entity (ADR 0025): Ethernet MDIO/MDC management interface (IEEE 802.3 clause 22/45).

use std::collections::BTreeMap;

use mosaic_contract::mdio::{MdioClause, MdioError, MdioInitiator, MdioTarget};
use mosaic_contract::model::Ctx;

/// MDIO bus entity: routes C22/C45 frames to the addressed PHY.
pub struct MdioBus {
    phys: BTreeMap<u8, Box<dyn MdioTarget>>,
}

impl Default for MdioBus { fn default() -> Self { Self::new() } }

impl MdioBus {
    pub fn new() -> Self { Self { phys: BTreeMap::new() } }
    pub fn add_phy(&mut self, prtad: u8, phy: Box<dyn MdioTarget>) {
        self.phys.insert(prtad, phy);
    }
    fn phy(&mut self, prtad: u8) -> Result<&mut Box<dyn MdioTarget>, MdioError> {
        self.phys.get_mut(&prtad).ok_or(MdioError::NoPhy)
    }
}

impl MdioInitiator for MdioBus {
    fn read(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, prtad: u8, devad: u8, reg: u8) -> Result<u16, MdioError> {
        self.phy(prtad)?.read(ctx, clause, devad, reg)
    }
    fn write(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, prtad: u8, devad: u8, reg: u8, value: u16) -> Result<(), MdioError> {
        self.phy(prtad)?.write(ctx, clause, devad, reg, value)
    }
}
