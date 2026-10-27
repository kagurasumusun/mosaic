//! PCA9535 16-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pca9535` — manifests/hw/gpio/pca9535.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::gpio::i2c_pcf857x::Pca9555;

/// pca9535: 2 bank(s) of input/output/polarity/config, strap 0x20.
pub fn new() -> Pca9555 {
    Pca9555::new(2).at(0x20)
}
