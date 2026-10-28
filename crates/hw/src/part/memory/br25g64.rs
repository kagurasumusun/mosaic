//! BR25G64 64Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g64` — manifests/hw/eeprom/br25g64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g64: 8192 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(8192, 2)
}
