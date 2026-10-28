//! BenQ FP2091 20.1" DVI LCD monitor (1600x1200)
//!
//! Catalog part — manifests/hw/display/benq-fp2091.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::part::display::dvi_monitor_core::DviMonitorCore;

/// benq-fp2091: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
