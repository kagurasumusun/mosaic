//! LG Philips LP150X07 15" XGA laptop panel (1024x768, LVDS)
//!
//! Catalog part — manifests/hw/display/lg-lp150x07.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::display::lvds_panel_core::LvdsPanelCore;

/// lg-lp150x07: native 1024x768, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1024, 768)
        .with_lanes(4)
}
