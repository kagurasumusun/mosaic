//! M95M01 1Mbit SPI EEPROM
//!
//! Catalog part `part:st/m95m01` — manifests/hw/eeprom/m95m01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95m01: 131072 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(131072, 2)
}
