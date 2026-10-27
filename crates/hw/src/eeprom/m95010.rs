//! M95010 1Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95010` — manifests/hw/eeprom/m95010.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95010: 128 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(128, 1)
}
