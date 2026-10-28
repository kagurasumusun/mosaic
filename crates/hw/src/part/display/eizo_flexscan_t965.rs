//! EIZO FlexScan T965 21" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/eizo-flexscan-t965.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// eizo-flexscan-t965: 350 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(350000)
}
