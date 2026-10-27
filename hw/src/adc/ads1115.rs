//! ADS1115 16-bit 4-channel I2C ADC
//!
//! Catalog part `part:ti/ads1115` — manifests/hw/adc/ads1115.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// ads1115: 4 16-bit registers, pointer mask 0x03, strap 0x48.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03)
        .at(0x48)
}
