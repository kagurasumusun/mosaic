//! M95128 128Kbit SPI EEPROM
//!
//! Catalog part `part:st/m95128` — manifests/hw/eeprom/m95128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// m95128: 16384 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(16384, 2)
}
