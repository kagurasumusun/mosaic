//! AT24C512 512Kbit I2C EEPROM
//!
//! Catalog part `part:microchip/at24c512` — manifests/hw/eeprom/at24c512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::i2c_24cxx::I2cEeprom;

/// at24c512: 65536 bytes, 128-byte page write, strap 0x50.
pub fn new() -> I2cEeprom {
    I2cEeprom::new(65536, 128).at(0x50)
}
