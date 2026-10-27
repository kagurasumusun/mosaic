//! BR24G01 1Kbit I2C EEPROM
//!
//! Catalog part `part:rohm/br24g01` — manifests/hw/eeprom/br24g01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// br24g01: 128 bytes, 8-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(128, 8).at(0x50)
}
