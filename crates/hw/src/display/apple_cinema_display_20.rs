//! Apple Cinema Display 20" DVI-D LCD monitor (1680x1050)
//!
//! Catalog part — manifests/hw/display/apple-cinema-display-20.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// apple-cinema-display-20: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
