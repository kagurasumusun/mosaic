//! Saitek X-36 gameport HOTAS
//!
//! Catalog part `part:saitek/saitek_x36` — manifests/hw/input/saitek_x36.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::input::gameport_stick_core::GamePortStickCore;

/// saitek_x36: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
