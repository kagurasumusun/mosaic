//! M95020 2Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95020` — manifests/hw/eeprom/m95020.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95020: 256 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(256, 1)
}
