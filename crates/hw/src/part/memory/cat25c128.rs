//! CAT25C128 128Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c128` — manifests/hw/eeprom/cat25c128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// cat25c128: 16384 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(16384, 2)
}
