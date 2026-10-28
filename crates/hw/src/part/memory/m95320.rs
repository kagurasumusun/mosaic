//! M95320 32Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95320` — manifests/hw/eeprom/m95320.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// m95320: 4096 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(4096, 2)
}
