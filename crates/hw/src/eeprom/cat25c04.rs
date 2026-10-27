//! CAT25C04 4Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c04` — manifests/hw/eeprom/cat25c04.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c04: 512 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(512, 2)
}
