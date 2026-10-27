//! M41T00 I2C serial real-time clock
//!
//! Catalog part `part:st/m41t00` — manifests/hw/rtc/m41t00.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::rtc::ds1307_core::Ds1307Core;

/// DS1307-layout RTC: BCD clock + 56-byte RAM, strap 0x68.
pub fn new() -> Ds1307Core {
    Ds1307Core::new().at(0x68)
}
