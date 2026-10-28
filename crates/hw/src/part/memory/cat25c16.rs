//! CAT25C16 16Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c16` — manifests/hw/eeprom/cat25c16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// cat25c16: 2048 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(2048, 2)
}
