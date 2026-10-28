//! Sony GDM-FW900 24" widescreen CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/sony-gdm-fw900.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// sony-gdm-fw900: 380 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(380000)
}
