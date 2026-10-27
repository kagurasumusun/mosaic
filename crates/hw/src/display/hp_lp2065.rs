//! HP LP2065 20.1" dual-input DVI LCD monitor (1600x1200)
//!
//! Catalog part — manifests/hw/display/hp-lp2065.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// hp-lp2065: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
