#![forbid(unsafe_code)]
//! `bus` — the interconnect foundation: one fabric per connection method.
//!
//! QEMU scatters its bus cores across `hw/i2c/core.c`, `hw/gpio/…` and each
//! controller's own wires; Mosaic consolidates every *connection method* into
//! this one crate so the taxonomy of "how do models talk to each other" is a
//! single tree (ADR 0025):
//!
//! | directory | connection method |
//! | --- | --- |
//! | `gpio/` | level-based pin nets (pulls, open-drain, collisions) |
//! | `i2c/` | 7-bit addressed two-wide transactions + bit-bang decode |
//! | `i3c/` | MIPI I3C: dynamic addressing (DAA), CCC, SDR transfers |
//! | `spi/` | chip-select full-duplex byte exchange |
//! | `can/` | identifier arbitration with a bounded deterministic queue |
//! | `uart/` | point-to-point UART pairs |
//! | `usb/` | USB ports, attach/detach and transfer routing |
//! | `net/` | Ethernet link frame exchange between NIC ports |
//! | `pci/` | PCI config-space routing (bus/device/function decode) |
//!
//! Rules that keep this foundation reusable (ADR 0025):
//!
//! * fabrics implement the port vocabularies from `contract`; the contracts
//!   stay in `contract` so neither a device nor an engine depends on fabrics;
//! * fabrics never depend on device models (`hw`), machines (`machine`) or
//!   composition (`assemble`/`runtime`) — a device that *speaks* a bus lives
//!   in `hw`, and the fabric it speaks *through* lives here;
//! * a fabric owns arbitration, addressing and delivery only; it never owns
//!   guest-visible registers — register files belong to the devices in `hw`.

pub mod ac97;
pub mod can;
pub mod canfd;
pub mod displayport;
pub mod dvi;
pub mod espi;
pub mod firewire;
pub mod flexray;
pub mod gameport;
pub mod gpib;
pub mod gpio;
pub mod hda;
pub mod hdmi;
pub mod hyperbus;
pub mod i2c;
pub mod i2s;
pub mod i3c;
pub mod isa;
pub mod jtag;
pub mod lpc;
pub mod lvds;
pub mod mdio;
pub mod microwire;
pub mod midi;
pub mod mipi_csi;
pub mod mipi_dsi;
pub mod nand;
pub mod net;
pub mod onewire;
pub mod ospi;
pub mod parallel;
pub mod pata;
pub mod pci;
pub mod pcie;
pub mod pcmcia;
pub mod pdm;
pub mod pmbus;
pub mod ps2;
pub mod pwm;
pub mod qspi;
pub mod rs485;
pub mod sata;
pub mod scsi;
pub mod sd;
pub mod sdio;
pub mod slimbus;
pub mod smbus;
pub mod soundwire;
pub mod spdif;
pub mod spi;
pub mod swd;
pub mod uart;
pub mod ufs;
pub mod usb;
pub mod vga;
