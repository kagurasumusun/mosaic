//! IS25WP128 128Mbit SPI flash (dual/quad)
//!
//! Catalog part `part:issi/is25wp128` — manifests/hw/eeprom/is25wp128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// is25wp128: 16777216 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(16777216, 2)
}
