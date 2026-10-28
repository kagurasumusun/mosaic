//! BR25G256 256Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g256` — manifests/hw/eeprom/br25g256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g256: 32768 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(32768, 2)
}
