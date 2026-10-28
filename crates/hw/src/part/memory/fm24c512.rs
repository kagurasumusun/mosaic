//! FM24C512 512Kbit F-RAM (I2C)
//!
//! Catalog part `part:cypress/fm24c512` — manifests/hw/eeprom/fm24c512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// fm24c512: 65536 bytes, 65536-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(65536, 65536).at(0x50)
}
