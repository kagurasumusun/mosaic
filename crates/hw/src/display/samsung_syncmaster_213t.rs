//! Samsung SyncMaster 213T 21.3" DVI LCD monitor (1600x1200)
//!
//! Catalog part — manifests/hw/display/samsung-syncmaster-213t.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// samsung-syncmaster-213t: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
