//! CAT24C08 8Kbit I2C EEPROM
//!
//! Catalog part `part:onsemi/cat24c08` — manifests/hw/eeprom/cat24c08.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// cat24c08: 1024 bytes, 16-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(1024, 16).at(0x50)
}
