//! LM75C digital temperature sensor (I2C, 9-bit)
//!
//! Catalog part `part:nxp/lm75c` — manifests/hw/sensor/lm75c.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// lm75c: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03)
        .at(0x48)
}
