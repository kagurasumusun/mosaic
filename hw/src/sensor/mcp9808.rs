//! MCP9808 ±0.5°C accuracy digital temperature sensor (I2C, 13-bit)
//!
//! Catalog part `part:microchip/mcp9808` — manifests/hw/sensor/mcp9808.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// mcp9808: 9 16-bit registers, pointer mask 0x0f, strap 0x18.
pub fn new() -> PointerSensor {
    PointerSensor::new(9, 0x0f)
        .with_register(6, 0x00, 0x54)
        .with_register(7, 0x04, 0x00)
        .at(0x18)
}
