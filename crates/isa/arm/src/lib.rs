#![forbid(unsafe_code)]
//! `target/arm` — the ARM/AArch32 target (QEMU's `target/arm`): the A32/T32
//! register vocabulary and the CPU core models (ARM926EJ-S, Cortex-M3).

pub mod cpu;
pub mod isa;

pub use isa::Register;
