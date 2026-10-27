//! PCF8570 256x8 static RAM with I2C
//!
//! Catalog part `part:nxp/pcf8570` — manifests/hw/eeprom/pcf8570.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// pcf8570: 256 bytes, 8-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(256, 8).at(0x50)
}
