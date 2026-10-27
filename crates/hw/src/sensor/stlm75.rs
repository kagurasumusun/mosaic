//! STLM75 digital temperature sensor (I2C, 11-bit)
//!
//! Catalog part `part:st/stlm75` — manifests/hw/sensor/stlm75.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// stlm75: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03).at(0x48)
}
