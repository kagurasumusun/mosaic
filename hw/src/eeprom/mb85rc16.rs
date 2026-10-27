//! MB85RC16 16Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc16` — manifests/hw/eeprom/mb85rc16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// mb85rc16: 2048 bytes, 2048-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(2048, 2048).at(0x50)
}
