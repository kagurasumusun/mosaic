//! M95080 8Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95080` — manifests/hw/eeprom/m95080.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// m95080: 1024 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(1024, 2)
}
