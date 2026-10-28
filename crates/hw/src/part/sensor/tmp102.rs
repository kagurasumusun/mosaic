//! TMP102 low-power digital temperature sensor (I2C, 12-bit)
//!
//! Catalog part `part:ti/tmp102` — manifests/hw/sensor/tmp102.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::modelkit::i2c_pointer::PointerSensor;

/// tmp102: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03).at(0x48)
}
