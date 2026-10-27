//! IS25LP032 32Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp032` — manifests/hw/eeprom/is25lp032.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// is25lp032: 4194304 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(4194304, 2)
}
