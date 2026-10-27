//! CAT25C256 256Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c256` — manifests/hw/eeprom/cat25c256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c256: 32768 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(32768, 2)
}
