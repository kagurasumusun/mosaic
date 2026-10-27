//! AT24CM02 2Mbit I2C EEPROM
//!
//! Catalog part `part:microchip/at24cm02` — manifests/hw/eeprom/at24cm02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// at24cm02: 262144 bytes, 256-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(262144, 256).at(0x50)
}
