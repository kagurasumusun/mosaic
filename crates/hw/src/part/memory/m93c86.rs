//! M93C86 16Kbit Microwire EEPROM
//!
//! Catalog part `part:st/m93c86` — manifests/hw/eeprom/m93c86.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::microwire_93cxx::MicrowireEeprom;

/// m93c86: 2048 bytes, 10-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(2048, 10)
}
