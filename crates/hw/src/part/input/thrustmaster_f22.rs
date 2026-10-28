//! ThrustMaster F-22 Pro gameport joystick
//!
//! Catalog part `part:thrustmaster/thrustmaster_f22` — manifests/hw/input/thrustmaster_f22.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// thrustmaster_f22: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
