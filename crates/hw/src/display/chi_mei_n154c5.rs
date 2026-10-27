//! Chi Mei N154C5-L05 15.4" WXGA laptop panel (1280x800, LVDS)
//!
//! Catalog part — manifests/hw/display/chi-mei-n154c5.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::display::lvds_panel_core::LvdsPanelCore;

/// chi-mei-n154c5: native 1280x800, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1280, 800)
        .with_lanes(4)
}
