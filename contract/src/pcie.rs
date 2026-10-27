//! `pcie` contract: PCI Express: TLP routing (distinct from legacy PCI config space).

use crate::model::Ctx;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PcieError {
    NoDevice,
    BadBdf,
    UnsupportedTlp,
    CompletionTimeout,
    Crc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlpType { MemRead, MemWrite, CfgRead0, CfgRead1, CfgWrite0, CfgWrite1, Msg, Cpl, CplD }

pub trait PcieInitiator {
    fn route(&mut self, ctx: &mut dyn Ctx, bdf: u16, tlp: TlpType, addr: u64, data: &[u8]) -> Result<Vec<u8>, PcieError>;
}

pub trait PcieTarget {
    fn bdf(&self) -> u16;
    fn route(&mut self, ctx: &mut dyn Ctx, tlp: TlpType, addr: u64, data: &[u8]) -> Result<Vec<u8>, PcieError>;
}
