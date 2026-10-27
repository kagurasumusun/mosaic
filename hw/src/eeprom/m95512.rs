//! M95512 512Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95512` — manifests/hw/eeprom/m95512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95512: 65536 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(65536, 2)
}
