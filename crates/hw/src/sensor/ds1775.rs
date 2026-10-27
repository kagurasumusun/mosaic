//! DS1775 digital thermometer and thermostat (I2C)
//!
//! Catalog part `part:maxim/ds1775` — manifests/hw/sensor/ds1775.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// ds1775: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03).at(0x48)
}
