//! MB85RC256 256Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc256` — manifests/hw/eeprom/mb85rc256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// mb85rc256: 32768 bytes, 32768-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(32768, 32768).at(0x50)
}
