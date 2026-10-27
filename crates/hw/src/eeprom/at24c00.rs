//! AT24C00 16-byte I2C EEPROM
//!
//! Catalog part `part:microchip/at24c00` — manifests/hw/eeprom/at24c00.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// at24c00: 16 bytes, 1-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(16, 1).at(0x50)
}
