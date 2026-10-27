//! Microsoft SideWinder 3D Pro gameport joystick
//!
//! Catalog part `part:microsoft/ms_sidewinder_3dpro` — manifests/hw/input/ms_sidewinder_3dpro.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::input::gameport_stick_core::GamePortStickCore;

/// ms_sidewinder_3dpro: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
