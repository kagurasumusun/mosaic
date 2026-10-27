//! `onewire` contract: Dallas/Maxim 1-Wire: single-pin half-duplex, ROM search + device commands.

use crate::model::Ctx;

/// 64-bit 1-Wire ROM address (family code + 48-bit serial + CRC8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneWireRom(pub u64);

impl OneWireRom {
    pub fn family(self) -> u8 { (self.0 & 0xFF) as u8 }
    pub fn crc_ok(self) -> bool { onewire_crc8(self.0) == 0 }
}

/// CRC8 of the 56-bit (family+serial) portion, polynomial 0x81 (reflected).
pub fn onewire_crc8(mut bits: u64) -> u8 {
    let mut crc: u8 = 0;
    for i in 0..56 {
        let mix = (crc ^ ((bits >> i) as u8 & 1)) & 1;
        crc >>= 1;
        if mix != 0 { crc ^= 0x8C; }
    }
    crc
}

/// 1-Wire error modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum OneWireError {
    NoPresence,
    Shorted,
    NotFound,
    CrcMismatch,
    Overflow,
    BusBusy,
}

/// Initiator-side port: a bus master drives 1-Wire protocol.
pub trait OneWireInitiator {
    fn reset(&mut self, ctx: &mut dyn Ctx) -> Result<bool, OneWireError>;
    fn write_bit(&mut self, ctx: &mut dyn Ctx, bit: bool) -> Result<(), OneWireError>;
    fn read_bit(&mut self, ctx: &mut dyn Ctx) -> Result<bool, OneWireError>;
    fn write_byte(&mut self, ctx: &mut dyn Ctx, byte: u8) -> Result<(), OneWireError> {
        for i in 0..8 {
            self.write_bit(ctx, (byte >> i) & 1 != 0)?;
        }
        Ok(())
    }
    fn read_byte(&mut self, ctx: &mut dyn Ctx) -> Result<u8, OneWireError> {
        let mut v = 0u8;
        for i in 0..8 {
            if self.read_bit(ctx)? { v |= 1 << i; }
        }
        Ok(v)
    }
}

/// Target-side port: a 1-Wire slave device responds to ROM matches and
/// function commands.
pub trait OneWireTarget {
    fn rom(&self) -> OneWireRom;
    fn matches(&self, rom: OneWireRom) -> bool;
    fn receive(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<(), OneWireError>;
    fn respond(&mut self, ctx: &mut dyn Ctx, len: usize) -> Result<Vec<u8>, OneWireError>;
}
