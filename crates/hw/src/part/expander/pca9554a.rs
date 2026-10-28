//! PCA9554A 16-bit I2C I/O expander
//!
//! Catalog part `part:nxp/pca9554a` — manifests/hw/gpio/pca9554a.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::expander::i2c_pcf857x::Pca9555;

/// pca9554a: 1 bank(s) of input/output/polarity/config, strap 0x38.
pub fn new() -> Pca9555 {
    Pca9555::new(1).at(0x38)
}
