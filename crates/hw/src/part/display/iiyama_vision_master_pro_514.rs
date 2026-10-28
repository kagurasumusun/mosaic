//! iiyama Vision Master Pro 514 (HM204DT) 22" CRT monitor (analog VGA)
//!
//! Catalog part — manifests/hw/display/iiyama-vision-master-pro-514.toml.
//! Thin configuration over the shared VGA monitor core (ADR 0029).

use crate::part::display::vga_monitor_core::VgaMonitorCore;

/// iiyama-vision-master-pro-514: 360 MHz video bandwidth
pub fn new() -> VgaMonitorCore {
    VgaMonitorCore::new().with_max_pixel_clock_khz(360000)
}
