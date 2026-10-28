//! Sharp LQ150X1LW71 15" XGA laptop panel (1024x768, LVDS)
//!
//! Catalog part — manifests/hw/display/sharp-lq150x1.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::part::display::lvds_panel_core::LvdsPanelCore;

/// sharp-lq150x1: native 1024x768, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1024, 768)
        .with_lanes(4)
}
