//! IDTech N150U3-L01 15" UXGA laptop panel (1600x1200, LVDS)
//!
//! Catalog part — manifests/hw/display/idtech-n150u3.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::display::lvds_panel_core::LvdsPanelCore;

/// idtech-n150u3: native 1600x1200, 8 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1600, 1200)
        .with_lanes(8)
}
