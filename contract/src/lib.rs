#![forbid(unsafe_code)]
//! `contract` — every port and bus vocabulary that models, machines and
//! engines speak, one module per contract (QEMU's `include/hw/*.h` role).
//! Contracts are traits and message types only; implementations live in
//! `hw` (devices and bus entities), `accel` (engines) and `runtime`.

pub mod can;
pub mod clock;
pub mod cpu_engine;
pub mod gpio;
pub mod i2c;
pub mod mmio;
pub mod model;
pub mod net;
pub mod signal;
pub mod spi;
pub mod stream;
pub mod uart_line;
