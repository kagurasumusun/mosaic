#![forbid(unsafe_code)]
//! `util` — leaf utilities with no VM vocabulary (QEMU's `util/`): bit
//! helpers, the byte FIFO device models share, and the FNV hasher.

pub mod bits;
pub mod fifo;
pub mod fnv;
