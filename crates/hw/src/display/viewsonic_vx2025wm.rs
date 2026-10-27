//! ViewSonic VX2025wm 20.1" widescreen DVI LCD monitor (1680x1050)
//!
//! Catalog part — manifests/hw/display/viewsonic-vx2025wm.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// viewsonic-vx2025wm: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
