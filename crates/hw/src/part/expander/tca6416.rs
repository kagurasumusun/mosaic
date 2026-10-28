//! TCA6416 16-bit I2C I/O expander
//!
//! Catalog part `part:ti/tca6416` — manifests/hw/gpio/tca6416.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::expander::i2c_pcf857x::Pca9555;

/// tca6416: 2 bank(s) of input/output/polarity/config, strap 0x20.
pub fn new() -> Pca9555 {
    Pca9555::new(2).at(0x20)
}
