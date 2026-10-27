//! M24C04 4Kbit I2C EEPROM
//!
//! Catalog part `part:st/m24c04` — manifests/hw/eeprom/m24c04.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// m24c04: 512 bytes, 16-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(512, 16).at(0x50)
}
