//! NEC AccuSync 95F 19" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/nec-accusync-95f.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// nec-accusync-95f: 210 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(210000)
}
