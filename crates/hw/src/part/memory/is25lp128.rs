//! IS25LP128 128Mbit SPI flash
//!
//! Catalog part `part:issi/is25lp128` — manifests/hw/eeprom/is25lp128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_25cxx::SpiEeprom;

/// is25lp128: 16777216 bytes, 2-byte address field.
pub fn new() -> SpiEeprom {
    SpiEeprom::new(16777216, 2)
}
