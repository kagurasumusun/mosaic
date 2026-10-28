//! TLA2024 12-bit 4-channel I2C ADC
//!
//! Catalog part `part:ti/tla2024` — manifests/hw/adc/tla2024.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::modelkit::i2c_pointer::PointerSensor;

/// tla2024: 2 16-bit registers, pointer mask 0x01, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(2, 0x01).at(0x48)
}
