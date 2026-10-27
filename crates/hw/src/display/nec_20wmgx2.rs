//! NEC 20WMGX2 20.1" widescreen DVI LCD monitor (1680x1050)
//!
//! Catalog part — manifests/hw/display/nec-20wmgx2.toml.
//! Thin configuration over the shared DVI monitor core (ADR 0029).

use crate::display::dvi_monitor_core::DviMonitorCore;

/// nec-20wmgx2: TMDS single-link
pub fn new() -> DviMonitorCore {
    DviMonitorCore::new().with_lanes(1)
}
