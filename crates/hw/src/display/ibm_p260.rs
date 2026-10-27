//! IBM P260 21" CRT monitor (analog VGA, Sony chassis)
//!
//! Catalog part — manifests/hw/display/ibm-p260.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// ibm-p260: 300 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(300000)
}
