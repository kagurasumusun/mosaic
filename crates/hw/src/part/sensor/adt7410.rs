//! ADT7410 16-bit digital temperature sensor (I2C)
//!
//! Catalog part `part:adi/adt7410` — manifests/hw/sensor/adt7410.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::modelkit::i2c_pointer::PointerSensor;

/// adt7410: 7 16-bit registers, pointer mask 0x07, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(7, 0x07).at(0x48)
}
