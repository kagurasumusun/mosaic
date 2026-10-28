//! Watchdog timer IP blocks (QEMU `hw/watchdog/`); external watchdog
//! chips live in `crate::part::rtc`.
pub mod catalog;
pub mod i6300esb;
pub mod iwdg_f1;
pub mod iwdg_f4;
pub mod iwdg_h7;
pub mod lpc11xx_wwdt;
pub mod rp2040_wdt;
pub mod sp805_wdt;
pub mod tco;
pub mod wwdg_f1;
