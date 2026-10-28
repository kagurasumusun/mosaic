//! Compaq P110 21" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/compaq-p110.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// compaq-p110: 300 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(300000)
}
