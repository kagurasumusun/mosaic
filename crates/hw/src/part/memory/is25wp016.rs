//! IS25WP016 16Mbit SPI flash (dual/quad)
//!
//! Catalog part `part:issi/is25wp016` — manifests/hw/eeprom/is25wp016.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25wp016: 2097152 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(2097152, 2)
}
