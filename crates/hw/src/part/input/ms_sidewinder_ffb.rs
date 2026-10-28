//! Microsoft SideWinder Force Feedback Pro gameport
//!
//! Catalog part `part:microsoft/ms_sidewinder_ffb` — manifests/hw/input/ms_sidewinder_ffb.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// ms_sidewinder_ffb: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
