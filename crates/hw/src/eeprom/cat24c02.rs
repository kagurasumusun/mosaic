//! CAT24C02 2Kbit I2C EEPROM
//!
//! Catalog part `part:onsemi/cat24c02` — manifests/hw/eeprom/cat24c02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// cat24c02: 256 bytes, 8-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(256, 8).at(0x50)
}
