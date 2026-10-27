//! BR25G04 4Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g04` — manifests/hw/eeprom/br25g04.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// br25g04: 512 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(512, 2)
}
