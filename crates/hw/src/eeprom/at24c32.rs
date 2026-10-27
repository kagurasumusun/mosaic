//! AT24C32 32Kbit I2C EEPROM
//!
//! Catalog part `part:microchip/at24c32` — manifests/hw/eeprom/at24c32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// at24c32: 4096 bytes, 32-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(4096, 32).at(0x50)
}
