//! BR25G32 32Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g32` — manifests/hw/eeprom/br25g32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g32: 4096 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(4096, 2)
}
