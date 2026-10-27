//! Sensor parts (QEMU `hw/sensor/`): bus-mediated targets. They attach to
//! I2C/SPI fabrics in board code — which address a sensor sits at is
//! board-unique wiring (ADR 0025).

pub mod bme280;
pub mod lm75a;
