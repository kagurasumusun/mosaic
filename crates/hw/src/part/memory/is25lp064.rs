//! IS25LP064 64Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp064` — manifests/hw/eeprom/is25lp064.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25lp064: 8388608 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(8388608, 2)
}
