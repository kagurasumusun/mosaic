//! M95M02 2Mbit SPI EEPROM
//!
//! Catalog part `part:st/m95m02` — manifests/hw/eeprom/m95m02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95m02: 262144 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(262144, 2)
}
