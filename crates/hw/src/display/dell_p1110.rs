//! Dell P1110 21" CRT monitor (analog VGA, Sony G520 chassis)
//!
//! Catalog part — manifests/hw/display/dell-p1110.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// dell-p1110: 340 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(340000)
}
