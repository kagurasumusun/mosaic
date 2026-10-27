//! LG L2000CP 20.1" DVI LCD monitor (1600x1200)
//!
//! Catalog part — manifests/hw/display/lg-l2000cp.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// lg-l2000cp: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
