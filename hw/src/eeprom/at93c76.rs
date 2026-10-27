//! AT93C76 8Kbit Microwire EEPROM
//!
//! Catalog part `part:microchip/at93c76` — manifests/hw/eeprom/at93c76.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::eeprom::microwire_93cxx::MicrowireEeprom;

/// at93c76: 1024 bytes, 9-bit address field.
pub fn new() -> MicrowireEeprom {
    MicrowireEeprom::new(1024, 9)
}
