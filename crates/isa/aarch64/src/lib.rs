#![forbid(unsafe_code)]
//! `target/aarch64` — the AArch64 target (QEMU's `target/arm` 64-bit half):
//! register vocabulary shared by machines and engines.

pub mod isa;

pub use isa::Register;
