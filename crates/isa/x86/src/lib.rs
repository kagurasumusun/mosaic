#![forbid(unsafe_code)]
//! `target/x86` — the x86 target (QEMU's `target/i386`, one crate for the
//! whole family: 16/32-bit `i386` and `x86_64`).

pub mod i386;
pub mod x86_64;
