//! LG Flatron F900B 19" flat-face CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/lg-flatron-f900b.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// lg-flatron-f900b: 250 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(250000)
}
