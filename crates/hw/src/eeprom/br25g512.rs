//! BR25G512 512Kbit SPI EEPROM
//!
//! Catalog part `part:rohm/br25g512` — manifests/hw/eeprom/br25g512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::spi_25cxx::SpiEeprom;

/// br25g512: 65536 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(65536, 2)
}
