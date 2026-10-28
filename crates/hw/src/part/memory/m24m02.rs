//! M24M02 2Mbit I2C EEPROM
//!
//! Catalog part `part:st/m24m02` — manifests/hw/eeprom/m24m02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// m24m02: 262144 bytes, 256-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(262144, 256).at(0x50)
}
