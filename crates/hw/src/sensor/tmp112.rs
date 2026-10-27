//! TMP112 high-accuracy digital temperature sensor (I2C, 12-bit)
//!
//! Catalog part `part:ti/tmp112` — manifests/hw/sensor/tmp112.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// tmp112: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03).at(0x48)
}
