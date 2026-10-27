#![forbid(unsafe_code)]
//! `contract` — every port and bus vocabulary that models, machines and
//! engines speak, one module per contract (QEMU's `include/hw/*.h` role).
//! Contracts are traits and message types only; implementations live in
//! `bus` (interconnect fabrics), `hw` (devices), `accel` (engines) and
//! `runtime`.

pub mod ac97;
pub mod can;
pub mod canfd;
pub mod clock;
pub mod cpu_engine;
pub mod dma;
pub mod ethernet;
pub mod flexray;
pub mod gpio;
pub mod hda;
pub mod i2c;
pub mod i2s;
pub mod i3c;
pub mod jtag;
pub mod lin;
pub mod lpc;
pub mod mdio;
pub mod microwire;
pub mod mmio;
pub mod model;
pub mod net;
pub mod onewire;
pub mod ospi;
pub mod parallel;
pub mod pata;
pub mod pci;
pub mod pcie;
pub mod pmbus;
pub mod qspi;
pub mod rs485;
pub mod sata;
pub mod scsi;
pub mod signal;
pub mod smbus;
pub mod spi;
pub mod stream;
pub mod swd;
pub mod uart_line;
pub mod usb;
