//! CAT25C02 2Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c02` — manifests/hw/eeprom/cat25c02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c02: 256 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(256, 1)
}
