//! Logitech ThunderPad Digital gameport
//!
//! Catalog part `part:logitech/logitech_thunderpad_digital` — manifests/hw/input/logitech_thunderpad_digital.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// logitech_thunderpad_digital: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
