//! BR24G02 2Kbit I2C EEPROM
//!
//! Catalog part `part:rohm/br24g02` — manifests/hw/eeprom/br24g02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// br24g02: 256 bytes, 8-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(256, 8).at(0x50)
}
