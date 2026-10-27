//! M24512 512Kbit I2C EEPROM
//!
//! Catalog part `part:st/m24512` — manifests/hw/eeprom/m24512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::i2c_24cxx::I2cEeprom;

/// m24512: 65536 bytes, 128-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(65536, 128).at(0x50)
}
