//! FM24C16 16Kbit F-RAM (I2C)
//!
//! Catalog part `part:cypress/fm24c16` — manifests/hw/eeprom/fm24c16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// fm24c16: 2048 bytes, 2048-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(2048, 2048).at(0x50)
}
