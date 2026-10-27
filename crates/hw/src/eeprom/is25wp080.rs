//! IS25WP080 8Mbit SPI flash (dual/quad)
//!
//! Catalog part `part:issi/is25wp080` — manifests/hw/eeprom/is25wp080.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// is25wp080: 1048576 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(1048576, 2)
}
