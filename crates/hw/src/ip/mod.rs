//! CPU-addressed device models — QEMU's `hw/<function>/` (ADR 0031).
//!
//! Every model here is placed into an address space by a board (`at` in
//! the machine manifest): SoC peripherals, host controllers, PCI/ISA cards.
//! A controller directory named after the fabric it masters (`i2c`, `spi`,
//! `usb`) is a *function* name, not a fabric classification — chips that
//! only a fabric reaches live in `crate::part`.

pub mod audio;
pub mod char;
pub mod clock;
pub mod display;
pub mod dma;
pub mod gpio;
pub mod i2c;
pub mod ide;
pub mod input;
pub mod intc;
pub mod mailbox;
pub mod net;
pub mod nvram;
pub mod pci;
pub mod platform;
pub mod rtc;
pub mod sd;
pub mod spi;
pub mod timer;
pub mod usb;
pub mod virtio;
pub mod wdt;
