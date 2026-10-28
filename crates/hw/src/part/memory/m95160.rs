//! M95160 16Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95160` — manifests/hw/eeprom/m95160.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// m95160: 2048 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(2048, 2)
}
