//! Samsung LTN154X3-L02 15.4" WXGA laptop panel (1280x800, LVDS)
//!
//! Catalog part — manifests/hw/display/samsung-ltn154x3.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::display::lvds_panel_core::LvdsPanelCore;

/// samsung-ltn154x3: native 1280x800, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1280, 800)
        .with_lanes(4)
}
