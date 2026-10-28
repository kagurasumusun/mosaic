//! AT24CM01 1Mbit I2C EEPROM
//!
//! Catalog part `part:microchip/at24cm01` — manifests/hw/eeprom/at24cm01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// at24cm01: 131072 bytes, 256-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(131072, 256).at(0x50)
}
