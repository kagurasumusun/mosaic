//! Sony SDM-HS95P 19" DVI LCD monitor (1280x1024)
//!
//! Catalog part — manifests/hw/display/sony-sdm-hs95p.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::part::display::dvi_monitor_core::DviMonitorCore;

/// sony-sdm-hs95p: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
