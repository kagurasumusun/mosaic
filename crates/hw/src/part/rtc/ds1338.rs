//! DS1338 I2C RTC with 56-byte NV RAM
//!
//! Catalog part `part:maxim/ds1338` — manifests/hw/rtc/ds1338.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::rtc::ds1307_core::Ds1307Core;

/// DS1307-layout RTC: BCD clock + 56-byte RAM, strap 0x68.
pub fn new() -> Ds1307Core {
    Ds1307Core::new().at(0x68)
}
