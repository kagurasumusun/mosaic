//! Dell UltraSharp 2001FP 20.1" DVI-D LCD monitor (1600x1200 UXGA)
//!
//! Catalog part — manifests/hw/display/dell-ultrasharp-2001fp.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::part::display::dvi_monitor_core::DviMonitorCore;

/// dell-ultrasharp-2001fp: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
