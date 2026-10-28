//! Logitech WingMan Extreme Digital gameport
//!
//! Catalog part `part:logitech/logitech_wingman_extreme` — manifests/hw/input/logitech_wingman_extreme.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// logitech_wingman_extreme: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
