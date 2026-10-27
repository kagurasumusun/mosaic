//! PCF8574 8-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pcf8574` — manifests/hw/gpio/pcf8574.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::gpio::i2c_pcf857x::Pcf857x;

/// pcf8574: quasi-bidirectional port, 1 byte(s), strap 0x20.
pub fn new() -> Pcf857x {
    Pcf857x::new(1).at(0x20)
}
