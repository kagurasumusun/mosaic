//! Guillemot Falcon gameport joystick
//!
//! Catalog part `part:guillemot/guillemot_falcon` — manifests/hw/input/guillemot_falcon.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// guillemot_falcon: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
