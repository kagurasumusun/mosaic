//! M95040 4Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95040` — manifests/hw/eeprom/m95040.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95040: 512 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(512, 2)
}
