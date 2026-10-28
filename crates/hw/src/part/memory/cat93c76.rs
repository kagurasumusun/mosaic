//! CAT93C76 8Kbit Microwire EEPROM
//!
//! Catalog part `part:onsemi/cat93c76` — manifests/hw/eeprom/cat93c76.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::microwire_93cxx::MicrowireEeprom;

/// cat93c76: 1024 bytes, 9-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(1024, 9)
}
