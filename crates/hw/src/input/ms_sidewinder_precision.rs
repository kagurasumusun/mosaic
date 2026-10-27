//! Microsoft SideWinder Precision Pro gameport
//!
//! Catalog part `part:microsoft/ms_sidewinder_precision` — manifests/hw/input/ms_sidewinder_precision.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::input::gameport_stick_core::GamePortStickCore;

/// ms_sidewinder_precision: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
