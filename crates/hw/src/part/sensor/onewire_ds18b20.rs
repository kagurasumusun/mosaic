//! DS18B20-family 1-Wire thermometer core (DS18B20/DS18S20/DS1822/DS1825/
//! MAX31820) — QEMU has no 1-Wire counterpart; this is the Maxim
//! datasheet's function-command set behind one shared core.
//!
//! After the fabric's ROM match (`Match ROM` / `Skip ROM`), the function
//! commands answer from a 9-byte scratchpad (temperature LSB/MSB, TH, TL,
//! config, three reserved `0xFF` bytes and a CRC8 in byte 8):
//!
//! * `Convert T (0x44)` refreshes the temperature bytes from the value
//!   injected through [`Ds18b20Core::set_temperature_c`] (the conversion
//!   is instantaneous in a functional model);
//! * `Read Scratchpad (0xBE)` arms the scratchpad stream;
//! * `Write Scratchpad (0x4E)` takes TH/TL/config (three bytes follow);
//! * `Copy Scratchpad (0x48)` and `Recall E2 (0xB8)` shuttle TH/TL/config
//!   between the scratchpad and the EEPROM copy;
//! * `Read Power Supply (0xB4)` answers 1 (external supply).

use mosaic_contract::model::Ctx;
use mosaic_contract::onewire::{OneWireError, OneWireRom, OneWireTarget};

mod command {
    pub const CONVERT_T: u8 = 0x44;
    pub const WRITE_SCRATCHPAD: u8 = 0x4E;
    pub const READ_SCRATCHPAD: u8 = 0xBE;
    pub const COPY_SCRATCHPAD: u8 = 0x48;
    pub const RECALL_E2: u8 = 0xB8;
    pub const READ_POWER_SUPPLY: u8 = 0xB4;
}

/// Scratchpad layout: temperature LSB, MSB, TH, TL, config, 3 reserved, CRC.
const SCRATCHPAD_LEN: usize = 9;
/// The resolution/config byte of a 12-bit conversion.
const CONFIG_12BIT: u8 = 0x7F;

/// One DS18B20-family thermometer.
pub struct Ds18b20Core {
    rom: OneWireRom,
    scratchpad: [u8; SCRATCHPAD_LEN],
    /// TH, TL, config — the EEPROM copy of the scratchpad trip bytes.
    eeprom: [u8; 3],
    /// Armed reply bytes (consumed by `respond`).
    armed: Vec<u8>,
}

impl Ds18b20Core {
    /// Family code (`0x28` DS18B20, `0x10` DS18S20, `0x22` DS1822,
    /// `0x3B` DS1825/MAX31820). The ROM serial is 1 with a valid CRC8.
    pub fn new(family: u8) -> Self {
        let mut part = Self {
            rom: OneWireRom(0),
            scratchpad: [0; SCRATCHPAD_LEN],
            eeprom: [0x4B, 0x46, CONFIG_12BIT], // TH 75C, TL 70C, 12-bit
            armed: Vec::new(),
        };
        part.rom = part.build_rom(family, 1);
        part.recall_e2();
        part.set_temperature_raw(0);
        part
    }

    /// A specific 48-bit serial (boards and tests).
    pub fn with_serial(mut self, serial: u64) -> Self {
        self.rom = self.build_rom(self.rom.family(), serial & 0xFFFF_FFFF_FFFF);
        self
    }

    fn build_rom(&self, family: u8, serial: u64) -> OneWireRom {
        let low = family as u64 | (serial << 8);
        let crc = mosaic_contract::onewire::onewire_crc8(low);
        OneWireRom(low | ((crc as u64) << 56))
    }

    /// The 64-bit ROM address.
    pub fn rom(&self) -> OneWireRom {
        self.rom
    }

    /// The scratchpad bytes (temperature through CRC).
    pub fn scratchpad(&self) -> &[u8; SCRATCHPAD_LEN] {
        &self.scratchpad
    }

    /// Inject the temperature in degrees Celsius (12-bit conversion grid,
    /// 0.0625 C per LSB; negative values use the 16-bit two's complement).
    pub fn set_temperature_c(&mut self, celsius: f32) {
        let raw = (celsius * 16.0) as i32;
        self.set_temperature_raw(raw.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
    }

    /// Inject the raw 16-bit temperature word (two's complement).
    pub fn set_temperature_raw(&mut self, raw: i16) {
        self.scratchpad[0] = raw as u8;
        self.scratchpad[1] = (raw >> 8) as u8;
        self.refresh_crc();
    }

    fn refresh_crc(&mut self) {
        // The Dallas 1-Wire CRC-8 (poly X^8+X^5+X^4+1, LSB-first input) —
        // the same CRC the ROM carries, over the first 8 scratchpad bytes.
        let mut crc: u8 = 0;
        for &byte in &self.scratchpad[..8] {
            let mut value = byte;
            for _ in 0..8 {
                let mix = (crc ^ value) & 1;
                crc >>= 1;
                if mix != 0 {
                    crc ^= 0x8C;
                }
                value >>= 1;
            }
        }
        self.scratchpad[8] = crc;
    }

    fn recall_e2(&mut self) {
        self.scratchpad[2] = self.eeprom[0];
        self.scratchpad[3] = self.eeprom[1];
        self.scratchpad[4] = self.eeprom[2];
        self.scratchpad[5] = 0xFF;
        self.scratchpad[6] = 0xFF;
        self.scratchpad[7] = 0xFF;
    }
}

impl OneWireTarget for Ds18b20Core {
    fn rom(&self) -> OneWireRom {
        self.rom
    }

    fn matches(&self, rom: OneWireRom) -> bool {
        rom == self.rom
    }

    fn receive(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<(), OneWireError> {
        let Some(&command) = bytes.first() else {
            return Ok(());
        };
        match command {
            command::CONVERT_T => {
                // The conversion is instantaneous: the raw word is already
                // injected, the CRC stays fresh.
            }
            command::READ_SCRATCHPAD => {
                self.armed = self.scratchpad.to_vec();
            }
            command::WRITE_SCRATCHPAD => {
                if bytes.len() >= 4 {
                    self.scratchpad[2] = bytes[1]; // TH
                    self.scratchpad[3] = bytes[2]; // TL
                    self.scratchpad[4] = bytes[3]; // config
                    self.refresh_crc();
                }
            }
            command::COPY_SCRATCHPAD => {
                self.eeprom = [self.scratchpad[2], self.scratchpad[3], self.scratchpad[4]];
            }
            command::RECALL_E2 => {
                self.recall_e2();
                self.refresh_crc();
            }
            command::READ_POWER_SUPPLY => {
                self.armed = vec![1]; // external supply
            }
            _ => {} // undefined function command: ignored
        }
        Ok(())
    }

    fn respond(&mut self, _ctx: &mut dyn Ctx, len: usize) -> Result<Vec<u8>, OneWireError> {
        let take = len.min(self.armed.len());
        let mut out: Vec<u8> = self.armed.drain(..take).collect();
        // Past the armed bytes the bus reads 1s.
        out.resize(len, 0xFF);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestCtx;

    fn part() -> Ds18b20Core {
        Ds18b20Core::new(0x28)
    }

    #[test]
    fn rom_carries_a_valid_family_and_crc() {
        let part = part();
        assert_eq!(part.rom().family(), 0x28);
        assert!(part.rom().crc_ok());
        assert!(Ds18b20Core::new(0x10).rom().crc_ok());
    }

    #[test]
    fn read_scratchpad_streams_the_nine_bytes_with_crc() {
        let mut part = part();
        part.set_temperature_c(25.0625); // raw 0x0191
        let mut ctx = TestCtx::default();
        part.receive(&mut ctx, &[0xBE]).unwrap();
        let out = part.respond(&mut ctx, 11).unwrap();
        assert_eq!(out[0], 0x91);
        assert_eq!(out[1], 0x01);
        assert_eq!(&out[5..8], &[0xFF, 0xFF, 0xFF]);
        assert_eq!(out[8], part.scratchpad()[8]);
        assert_eq!(&out[9..], &[0xFF, 0xFF]); // past the scratchpad: 1s
    }

    #[test]
    fn crc_matches_the_datasheet_power_on_scratchpad() {
        let mut part = part();
        part.set_temperature_c(85.0); // power-on value 0x0550
        assert_eq!(part.scratchpad()[0], 0x50);
        assert_eq!(part.scratchpad()[1], 0x05);
        // Power-on scratchpad 50 05 4B 46 7F FF FF FF -> CRC 0x78 (the
        // Dallas 1-Wire CRC-8, validated against the AN27 DS2401 vector).
        assert_eq!(part.scratchpad()[8], 0x78);
    }

    #[test]
    fn negative_temperatures_use_twos_complement() {
        let mut part = part();
        part.set_temperature_c(-10.125); // raw -162 = 0xFF5E
        assert_eq!(part.scratchpad()[0], 0x5E);
        assert_eq!(part.scratchpad()[1], 0xFF);
    }

    #[test]
    fn write_copy_and_recall_shuttle_the_trip_bytes() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        part.receive(&mut ctx, &[0x4E, 0x30, 0x28, 0x7F]).unwrap();
        assert_eq!(&part.scratchpad()[2..5], &[0x30, 0x28, 0x7F]);
        part.receive(&mut ctx, &[0x48]).unwrap(); // copy to EEPROM
        part.receive(&mut ctx, &[0x4E, 0x00, 0x00, 0x00]).unwrap();
        part.receive(&mut ctx, &[0xB8]).unwrap(); // recall from EEPROM
        assert_eq!(&part.scratchpad()[2..5], &[0x30, 0x28, 0x7F]);
    }

    #[test]
    fn read_power_supply_answers_external() {
        let mut part = part();
        let mut ctx = TestCtx::default();
        part.receive(&mut ctx, &[0xB4]).unwrap();
        assert_eq!(part.respond(&mut ctx, 1).unwrap(), vec![1]);
    }

    #[test]
    fn only_the_matching_rom_answers() {
        let part = part();
        let other = Ds18b20Core::new(0x28).with_serial(2);
        assert!(part.matches(part.rom()));
        assert!(!part.matches(other.rom()));
    }
}
