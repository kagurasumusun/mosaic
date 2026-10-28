//! Sony CPD-G520 21" CRT monitor (analog VGA, 300 MHz video)
//!
//! Catalog part — manifests/hw/display/sony-cpd-g520.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// sony-cpd-g520: 300 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(300000)
}
