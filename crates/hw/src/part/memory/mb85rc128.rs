//! MB85RC128 128Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc128` — manifests/hw/eeprom/mb85rc128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// mb85rc128: 16384 bytes, 16384-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(16384, 16384).at(0x50)
}
