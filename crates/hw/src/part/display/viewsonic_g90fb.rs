//! ViewSonic G90fb 19" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/viewsonic-g90fb.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// viewsonic-g90fb: 260 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(260000)
}
