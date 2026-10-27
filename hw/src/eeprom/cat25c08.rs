//! CAT25C08 8Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c08` — manifests/hw/eeprom/cat25c08.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c08: 1024 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(1024, 2)
}
