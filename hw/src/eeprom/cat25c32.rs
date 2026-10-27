//! CAT25C32 32Kbit SPI EEPROM
//!
//! Catalog part `part:onsemi/cat25c32` — manifests/hw/eeprom/cat25c32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// cat25c32: 4096 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(4096, 2)
}
