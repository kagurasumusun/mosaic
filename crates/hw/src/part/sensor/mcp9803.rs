//! MCP9803 digital temperature sensor (I2C, 12-bit)
//!
//! Catalog part `part:microchip/mcp9803` — manifests/hw/sensor/mcp9803.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::modelkit::i2c_pointer::PointerSensor;

/// mcp9803: 4 16-bit registers, pointer mask 0x03, strap 0x18.
pub fn new() -> PointerSensor {
    PointerSensor::new(4, 0x03).at(0x18)
}
