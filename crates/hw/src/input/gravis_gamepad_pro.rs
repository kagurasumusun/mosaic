//! Gravis GamePad Pro gameport
//!
//! Catalog part `part:gravis/gravis_gamepad_pro` — manifests/hw/input/gravis_gamepad_pro.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::input::gameport_stick_core::GamePortStickCore;

/// gravis_gamepad_pro: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
