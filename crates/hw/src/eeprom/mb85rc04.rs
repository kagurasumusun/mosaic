//! MB85RC04 4Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc04` — manifests/hw/eeprom/mb85rc04.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// mb85rc04: 512 bytes, 512-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(512, 512).at(0x50)
}
