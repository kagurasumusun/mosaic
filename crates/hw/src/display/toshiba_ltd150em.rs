//! Toshiba-Matsushita LTD150EM06 15" XGA laptop panel (1024x768, LVDS)
//!
//! Catalog part — manifests/hw/display/toshiba-ltd150em.toml.
//! Thin configuration over the shared LVDS panel core (ADR 0029).

use crate::display::lvds_panel_core::LvdsPanelCore;

/// toshiba-ltd150em: native 1024x768, 4 data lanes
pub fn new() -> LvdsPanelCore {
    LvdsPanelCore::new()
        .with_resolution(1024, 768)
        .with_lanes(4)
}
