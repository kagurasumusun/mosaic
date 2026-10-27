//! CAT25C64 64Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c64` — manifests/hw/eeprom/cat25c64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c64: 8192 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(8192, 2)
}
