//! MB85RC64 64Kbit FRAM (I2C)
//!
//! Catalog part `part:fujitsu/mb85rc64` — manifests/hw/eeprom/mb85rc64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// mb85rc64: 8192 bytes, 8192-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(8192, 8192).at(0x50)
}
