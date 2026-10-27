//! MCP9804 digital temperature sensor (I2C, 13-bit)
//!
//! Catalog part `part:microchip/mcp9804` — manifests/hw/sensor/mcp9804.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::i2c_pointer::PointerSensor;

/// mcp9804: 9 16-bit registers, pointer mask 0x0f, strap 0x18.
pub fn new() -> PointerSensor {
    PointerSensor::new(9, 0x0f).at(0x18)
}
