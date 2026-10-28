//! BR25G01 1Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g01` — manifests/hw/eeprom/br25g01.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g01: 128 bytes, 1-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(128, 1)
}
