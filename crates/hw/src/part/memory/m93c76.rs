//! M93C76 8Kbit Microwire EEPROM
//!
//! Catalog part `part:st/m93c76` — manifests/hw/eeprom/m93c76.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::microwire_93cxx::MicrowireEeprom;

/// m93c76: 1024 bytes, 9-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(1024, 9)
}
