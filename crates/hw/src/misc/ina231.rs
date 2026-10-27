//! TI INA231 voltage/current monitor (I2C, 16-bit)
//!
//! Catalog part `part:ti/ina231` — manifests/hw/misc/ina231.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// ina231: 6 16-bit registers, pointer mask 0x07, strap 0x40.
pub fn new() -> PointerSensor {
    PointerSensor::new(6, 0x07).at(0x40)
}
