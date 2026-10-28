//! FM24V10 1Mbit F-RAM (I2C)
//!
//! Catalog part `part:cypress/fm24v10` — manifests/hw/eeprom/fm24v10.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// fm24v10: 131072 bytes, 131072-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(131072, 131072).at(0x50)
}
