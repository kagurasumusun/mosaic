//! CAT93C66 4Kbit Microwire EEPROM
//!
//! Catalog part `part:onsemi/cat93c66` — manifests/hw/eeprom/cat93c66.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::microwire_93cxx::MicrowireEeprom;

/// cat93c66: 512 bytes, 8-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(512, 8)
}
