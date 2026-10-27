//! AT93C56 2Kbit Microwire EEPROM
//!
//! Catalog part `part:microchip/at93c56` — manifests/hw/eeprom/at93c56.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::microwire_93cxx::MicrowireEeprom;

/// at93c56: 256 bytes, 8-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(256, 8)
}
