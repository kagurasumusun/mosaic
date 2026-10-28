//! PCA8581 128x8 I2C EEPROM
//!
//! Catalog part `part:nxp/pca8581` — manifests/hw/eeprom/pca8581.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// pca8581: 128 bytes, 8-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(128, 8).at(0x50)
}
