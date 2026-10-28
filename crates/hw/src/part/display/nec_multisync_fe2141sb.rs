//! NEC MultiSync FE2141SB 22" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/nec-multisync-fe2141sb.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// nec-multisync-fe2141sb: 345 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(345000)
}
