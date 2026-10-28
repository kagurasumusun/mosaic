//! BR24G04 4Kbit I2C EEPROM
//!
//! Catalog part `part:rohm/br24g04` — manifests/hw/eeprom/br24g04.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// br24g04: 512 bytes, 16-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(512, 16).at(0x50)
}
