//! PCF8574A 8-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pcf8574a` — manifests/hw/gpio/pcf8574a.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::gpio::i2c_pcf857x::Pcf857x;

/// pcf8574a: quasi-bidirectional port, 1 byte(s), strap 0x38.
pub fn new() -> Pcf857x {
    Pcf857x::new(1).at(0x38)
}
