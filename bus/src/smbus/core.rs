//! `smbus` bus entity (ADR 0025): SMBus (System Management Bus): I2C-derived, with ARP/PEC/alert.

use std::collections::BTreeMap;
use mosaic_contract::model::Ctx;
use mosaic_contract::smbus::{SmbusError, SmbusInitiator, SmbusTarget};

pub struct SmbusBus { targets: BTreeMap<u8, Box<dyn SmbusTarget>> }

impl Default for SmbusBus { fn default() -> Self { Self::new() } }

impl SmbusBus {
    pub fn new() -> Self { Self { targets: BTreeMap::new() } }
    pub fn add_target(&mut self, t: Box<dyn SmbusTarget>) { self.targets.insert(t.address(), t); }
    fn tgt(&mut self, addr: u8) -> Result<&mut Box<dyn SmbusTarget>, SmbusError> {
        self.targets.get_mut(&addr).ok_or(SmbusError::Nack)
    }
}

impl SmbusInitiator for SmbusBus {
    fn quick_command(&mut self, _ctx: &mut dyn Ctx, addr: u8, _read: bool) -> Result<(), SmbusError> {
        self.tgt(addr).map(|_| ())
    }
    fn send_byte(&mut self, ctx: &mut dyn Ctx, addr: u8, byte: u8) -> Result<(), SmbusError> {
        self.tgt(addr)?.receive(ctx, &[byte])
    }
    fn receive_byte(&mut self, ctx: &mut dyn Ctx, addr: u8) -> Result<u8, SmbusError> {
        let v = self.tgt(addr)?.respond(ctx, 1)?;
        Ok(v.first().copied().unwrap_or(0))
    }
    fn read_block(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8) -> Result<Vec<u8>, SmbusError> {
        self.tgt(addr)?.receive(ctx, &[cmd])?;
        self.tgt(addr)?.respond(ctx, 32)
    }
    fn process_call(&mut self, ctx: &mut dyn Ctx, addr: u8, cmd: u8, value: u16) -> Result<u16, SmbusError> {
        let bytes = [cmd, (value & 0xFF) as u8, (value >> 8) as u8];
        self.tgt(addr)?.receive(ctx, &bytes)?;
        let resp = self.tgt(addr)?.respond(ctx, 2)?;
        Ok(resp.get(0).copied().unwrap_or(0) as u16 | ((resp.get(1).copied().unwrap_or(0) as u16) << 8))
    }
}
