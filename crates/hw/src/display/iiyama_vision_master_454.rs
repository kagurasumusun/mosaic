//! iiyama Vision Master 454 (HM903DT) 19" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/iiyama-vision-master-454.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::display::vga_monitor_core::VgaMonitorCore;

/// iiyama-vision-master-454: 300 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(300000)
}
