//! MB85RC01 1Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc01` — manifests/hw/eeprom/mb85rc01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// mb85rc01: 128 bytes, 128-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(128, 128).at(0x50)
}
