//! FM24C256 256Kbit F-RAM (I2C)
//!
//! Catalog part `part:cypress/fm24c256` — manifests/hw/eeprom/fm24c256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// fm24c256: 32768 bytes, 32768-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(32768, 32768).at(0x50)
}
