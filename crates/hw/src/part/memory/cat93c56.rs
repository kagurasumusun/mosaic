//! CAT93C56 2Kbit Microwire EEPROM
//!
//! Catalog part `part:onsemi/cat93c56` — manifests/hw/eeprom/cat93c56.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::microwire_93cxx::MicrowireEeprom;

/// cat93c56: 256 bytes, 8-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(256, 8)
}
