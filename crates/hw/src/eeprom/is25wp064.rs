//! IS25WP064 64Mbit SPI flash (dual/quad)
//!
//! Catalog part `part:issi/is25wp064` — manifests/hw/eeprom/is25wp064.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// is25wp064: 8388608 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(8388608, 2)
}
