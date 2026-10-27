//! IS25WP032 32Mbit SPI flash (dual/quad)
//!
//! Catalog part `part:issi/is25wp032` — manifests/hw/eeprom/is25wp032.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// is25wp032: 4194304 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(4194304, 2)
}
