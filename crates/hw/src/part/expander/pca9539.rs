//! PCA9539 16-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pca9539` — manifests/hw/gpio/pca9539.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::expander::i2c_pcf857x::Pca9555;

/// pca9539: 2 bank(s) of input/output/polarity/config, strap 0x74.
pub fn new() -> Pca9555 {
    Pca9555::new(2).at(0x74)
}
