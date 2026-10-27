#![forbid(unsafe_code)]
//! `hw` — device models, organized the way QEMU organizes its `hw/`: one
//! directory per function (char, intc, timer, gpio, i2c, ssi, ...), one file
//! — or one directory for multi-bus models — per device, the vendor as a
//! name prefix rather than a directory axis (ADR 0024).
//!
//! Bus entities (the I2C/SPI/CAN bus cores, the GPIO net, the uart pair)
//! live beside the devices that speak them, exactly like QEMU's
//! `hw/i2c/core.c`. Devices never depend on machines; the `machine` crate
//! depends on this crate, never the other way around.

pub mod can;
pub mod char;
pub mod gpio;
pub mod i2c;
pub mod ide;
pub mod intc;
pub mod mailbox;
pub mod misc;
pub mod nvram;
pub mod pci;
pub mod rtc;
pub mod sensor;
pub mod ssi;
pub mod timer;
pub mod virtio;
