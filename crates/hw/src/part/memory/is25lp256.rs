//! IS25LP256 256Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp256` — manifests/hw/eeprom/is25lp256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25lp256: 33554432 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(33554432, 2)
}
