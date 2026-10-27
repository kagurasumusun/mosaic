//! `mdio` contract: Ethernet MDIO/MDC management interface (IEEE 802.3 clause 22/45).

use crate::model::Ctx;

/// MDIO error modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MdioError {
    NoPhy,
    BadClause,
    BadPrtad,
    BadDevad,
    BusBusy,
    ReadTimeout,
}

/// MDIO clause: 22 (5-bit reg) or 45 (device-mode addressing).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MdioClause { C22, C45 }

/// Initiator-side port: an MAC drives the MDIO bus.
pub trait MdioInitiator {
    fn read(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, prtad: u8, devad: u8, reg: u8) -> Result<u16, MdioError>;
    fn write(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, prtad: u8, devad: u8, reg: u8, value: u16) -> Result<(), MdioError>;
}

/// Target-side port: a PHY responds to MDIO.
pub trait MdioTarget {
    fn prtad(&self) -> u8;
    fn read(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, devad: u8, reg: u8) -> Result<u16, MdioError>;
    fn write(&mut self, ctx: &mut dyn Ctx, clause: MdioClause, devad: u8, reg: u8, value: u16) -> Result<(), MdioError>;
}
