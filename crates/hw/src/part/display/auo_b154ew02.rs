//! AUO B154EW02-V0 15.4" WXGA laptop panel (1280x800, LVDS)
//!
//! Catalog part — manifests/hw/display/auo-b154ew02.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::part::display::lvds_panel_core::LvdsPanelCore;

/// auo-b154ew02: native 1280x800, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1280, 800)
        .with_lanes(4)
}
