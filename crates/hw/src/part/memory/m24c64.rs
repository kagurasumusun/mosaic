//! M24C64 64Kbit I2C EEPROM
//!
//! Catalog part `part:st/m24c64` — manifests/hw/eeprom/m24c64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// m24c64: 8192 bytes, 32-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(8192, 32).at(0x50)
}
