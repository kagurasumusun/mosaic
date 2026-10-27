//! MAX30205 human body temperature sensor (I2C, 16-bit)
//!
//! Catalog part `part:maxim/max30205` — manifests/hw/sensor/max30205.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// max30205: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03)
        .at(0x48)
}
