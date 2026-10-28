//! BR25G08 8Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g08` — manifests/hw/eeprom/br25g08.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// br25g08: 1024 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(1024, 2)
}
