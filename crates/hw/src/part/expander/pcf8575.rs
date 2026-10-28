//! PCF8575 16-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pcf8575` — manifests/hw/gpio/pcf8575.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::expander::i2c_pcf857x::Pcf857x;

/// pcf8575: quasi-bidirectional port, 2 byte(s), strap 0x20.
pub fn new() -> Pcf857x {
    Pcf857x::new(2).at(0x20)
}
