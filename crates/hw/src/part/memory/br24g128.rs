//! BR24G128 128Kbit I2C EEPROM
//!
//! Catalog part `part:rohm/br24g128` — manifests/hw/eeprom/br24g128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// br24g128: 16384 bytes, 64-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(16384, 64).at(0x50)
}
