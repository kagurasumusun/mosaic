//! AT24C16 16Kbit I2C EEPROM
//!
//! Catalog part `part:microchip/at24c16` — manifests/hw/eeprom/at24c16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// at24c16: 2048 bytes, 16-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(2048, 16).at(0x50)
}
