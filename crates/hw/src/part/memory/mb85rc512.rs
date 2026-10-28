//! MB85RC512 512Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc512` — manifests/hw/eeprom/mb85rc512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// mb85rc512: 65536 bytes, 65536-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(65536, 65536).at(0x50)
}
