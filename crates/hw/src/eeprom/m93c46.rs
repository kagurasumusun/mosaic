//! M93C46 1Kbit Microwire EEPROM
//!
//! Catalog part `part:st/m93c46` — manifests/hw/eeprom/m93c46.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::microwire_93cxx::MicrowireEeprom;

/// m93c46: 128 bytes, 7-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(128, 7)
}
