//! USB interconnect: ports, attach/detach and transfer routing.
//!
//! The fabric is the tier between a host controller model (`hw/usb/uhci.rs`)
//! and the attached device models. It owns exactly what a USB bus owns:
//! the port list, the device addressing (SET_ADDRESS bookkeeping), and the
//! routing of transfer phases to the addressed device. Endpoint state,
//! descriptors and stall semantics belong to each device.
//!
//! Controllers do not talk to devices directly — the same rule every other
//! fabric in this crate enforces, so any controller can drive any device
//! (and a hub, when it arrives, is just a controller-facing device with
//! downstream ports of its own).

pub mod core;
