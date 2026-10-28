//! TI INA226 voltage/current monitor (I2C, 16-bit)
//!
//! Catalog part `part:ti/ina226` — manifests/hw/misc/ina226.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::modelkit::i2c_pointer::PointerSensor;

/// ina226: 6 16-bit registers, pointer mask 0x07, strap 0x40.
pub fn new() -> PointerSensor {
    PointerSensor::new(6, 0x07).at(0x40)
}
