//! STTS751 digital temperature sensor (I2C, 12-bit)
//!
//! Catalog part `part:st/stts751` — manifests/hw/sensor/stts751.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// stts751: 5 16-bit registers, pointer mask 0x07, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(5, 0x07)
        .at(0x48)
}
