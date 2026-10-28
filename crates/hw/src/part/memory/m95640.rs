//! M95640 64Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95640` — manifests/hw/eeprom/m95640.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// m95640: 8192 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(8192, 2)
}
