//! `onewire` contract: Dallas/Maxim 1-Wire: single-pin half-duplex, ROM search + device commands.

use crate::model::Ctx;

/// 64-bit 1-Wire ROM address (family code + 48-bit serial + CRC8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OneWireRom(pub u64);

impl OneWireRom {
    pub fn family(self) -> u8 {
        (self.0 & 0xFF) as u8
    }
    /// The 48-bit serial between the family code and the CRC.
    pub fn serial(self) -> u64 {
        (self.0 >> 8) & 0xFFFF_FFFF_FFFF
    }
    /// The CRC8 over the family code and serial (the low 56 bits).
    pub fn crc8(self) -> u8 {
        onewire_crc8(self.0)
    }
    /// The stored CRC byte (bits 56..64) equals the computed CRC.
    pub fn crc_ok(self) -> bool {
        self.crc8() == ((self.0 >> 56) & 0xFF) as u8
    }
}

/// CRC8 of the 56-bit (family+serial) portion, polynomial 0x81 (reflected).
pub fn onewire_crc8(mut bits: u64) -> u8 {
    let mut crc: u8 = 0;
    for i in 0..56 {
        let mix = (crc ^ ((bits >> i) as u8 & 1)) & 1;
        crc >>= 1;
        if mix != 0 {
            crc ^= 0x8C;
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Maxim AN27 worked example: ROM 02 1C B8 01 00 00 00 with CRC A2.
    #[test]
    fn an27_rom_vector_validates() {
        let rom = OneWireRom(0x02 | 0x1C << 8 | 0xB8 << 16 | 0x01 << 24 | 0xA2 << 56);
        assert_eq!(rom.family(), 0x02);
        assert_eq!(rom.serial(), 0x0000_0001_B81C);
        assert_eq!(rom.crc8(), 0xA2);
        assert!(rom.crc_ok());
    }

    #[test]
    fn a_corrupted_crc_is_rejected() {
        let rom = OneWireRom(0x02 | 0x1C << 8 | 0xB8 << 16 | 0x01 << 24 | 0xA3 << 56);
        assert!(!rom.crc_ok());
    }
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
