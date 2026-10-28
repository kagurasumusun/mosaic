//! FM24C64 64Kbit F-RAM (I2C)
//!
//! Catalog part `part:cypress/fm24c64` — manifests/hw/eeprom/fm24c64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// fm24c64: 8192 bytes, 8192-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(8192, 8192).at(0x50)
}
