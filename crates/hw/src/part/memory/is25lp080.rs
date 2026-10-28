//! IS25LP080 8Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp080` — manifests/hw/eeprom/is25lp080.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25lp080: 1048576 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(1048576, 2)
}
