//! IS25LP016 16Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp016` — manifests/hw/eeprom/is25lp016.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25lp016: 2097152 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(2097152, 2)
}
