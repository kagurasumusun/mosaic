//! `pmbus` bus entity (ADR 0025): PMBus (Power Management Bus): SMBus-derived, power-supply commands.

use mosaic_contract::model::Ctx;
use mosaic_contract::pmbus::{PmbusError, PmbusInitiator, PmbusTarget};
use std::collections::BTreeMap;

pub struct PmbusBus {
    targets: BTreeMap<u8, Box<dyn PmbusTarget>>,
}

impl Default for PmbusBus {
    fn default() -> Self {
        Self::new()
    }
}

impl PmbusBus {
    pub fn new() -> Self {
        Self {
            targets: BTreeMap::new(),
        }
    }
    pub fn add_target(&mut self, t: Box<dyn PmbusTarget>) {
        self.targets.insert(t.address(), t);
    }
    fn tgt(&mut self, addr: u8) -> Result<&mut Box<dyn PmbusTarget>, PmbusError> {
        self.targets.get_mut(&addr).ok_or(PmbusError::Nack)
    }
}

impl PmbusInitiator for PmbusBus {
    fn read_byte(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<u8, PmbusError> {
        let mut buf = Vec::new();
        self.tgt(addr)?.command(ctx, cmd, false, &mut buf)?;
        Ok(buf.first().copied().unwrap_or(0))
    }
    fn read_word(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<u16, PmbusError> {
        let mut buf = Vec::new();
        self.tgt(addr)?.command(ctx, cmd, false, &mut buf)?;
        Ok(buf.get(0).copied().unwrap_or(0) as u16
            | ((buf.get(1).copied().unwrap_or(0) as u16) << 8))
    }
    fn read_block(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<Vec<u8>, PmbusError> {
        let mut buf = Vec::new();
        self.tgt(addr)?.command(ctx, cmd, false, &mut buf)?;
        Ok(buf)
    }
    fn write_byte(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        value: u8,
    ) -> Result<(), PmbusError> {
        let mut buf = vec![value];
        self.tgt(addr)?.command(ctx, cmd, true, &mut buf)
    }
    fn write_word(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        value: u16,
    ) -> Result<(), PmbusError> {
        let mut buf = vec![(value & 0xFF) as u8, (value >> 8) as u8];
        self.tgt(addr)?.command(ctx, cmd, true, &mut buf)
    }
    fn write_block(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u8,
        cmd: u8,
        data: &[u8],
    ) -> Result<(), PmbusError> {
        let mut buf = data.to_vec();
        self.tgt(addr)?.command(ctx, cmd, true, &mut buf)
    }
}
