//! BR25G02 2Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g02` — manifests/hw/eeprom/br25g02.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g02: 256 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(256, 1)
}
