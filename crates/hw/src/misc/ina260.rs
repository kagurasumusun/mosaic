//! TI INA260 integrated shunt voltage/current monitor (I2C)
//!
//! Catalog part `part:ti/ina260` — manifests/hw/misc/ina260.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// ina260: 6 16-bit registers, pointer mask 0x07, strap 0x40.
pub fn new() -> PointerSensor {
    PointerSensor::new(6, 0x07).at(0x40)
}
