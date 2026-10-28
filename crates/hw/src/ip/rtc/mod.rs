//! On-SoC real-time clock IP blocks (QEMU `hw/rtc/`); external RTC
//! chips live in `crate::part::rtc`.
pub mod am335x_rtc;
pub mod catalog;
pub mod lpc11xx_rtc;
pub mod lpc17xx_rtc;
pub mod mc146818;
pub mod pl031;
pub mod rp2040_rtc;
pub mod rtc_f1;
pub mod rtc_f4;
pub mod rtc_h7;
pub mod rtc_l4;
