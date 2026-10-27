//! Mitsubishi Diamond Pro 2070SB 22" CRT monitor (analog VGA, aperture grille)
//!
//! Catalog part — manifests/hw/display/mitsubishi-diamond-pro-2070sb.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// mitsubishi-diamond-pro-2070sb: 370 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(370000)
}
