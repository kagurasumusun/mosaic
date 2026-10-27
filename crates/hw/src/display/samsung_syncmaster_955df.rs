//! Samsung SyncMaster 955df 19" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/samsung-syncmaster-955df.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// samsung-syncmaster-955df: 240 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(240000)
}
