//! HP P1130 21" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/hp-p1130.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// hp-p1130: 340 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(340000)
}
