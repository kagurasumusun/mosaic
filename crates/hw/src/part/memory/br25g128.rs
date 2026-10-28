//! BR25G128 128Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g128` — manifests/hw/eeprom/br25g128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g128: 16384 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(16384, 2)
}
