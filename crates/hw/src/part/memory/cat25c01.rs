//! CAT25C01 1Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c01` — manifests/hw/eeprom/cat25c01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// cat25c01: 128 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(128, 1)
}
