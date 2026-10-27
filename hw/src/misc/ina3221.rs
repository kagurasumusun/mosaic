//! TI INA3221 3-channel voltage/current monitor (I2C, 16-bit)
//!
//! Catalog part `part:ti/ina3221` — manifests/hw/misc/ina3221.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// ina3221: 9 16-bit registers, pointer mask 0x0f, strap 0x40.
pub fn new() -> PointerSensor {
    PointerSensor::new(9, 0x0f)
        .at(0x40)
}
