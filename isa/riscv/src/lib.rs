#![forbid(unsafe_code)]
//! `target/riscv` — the RISC-V target (QEMU's `target/riscv`): the RV32/RV64
//! register vocabulary and CPU core models (SiFive E31).

pub mod cpu;
pub mod isa;

pub use isa::Register;
