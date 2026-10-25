#![no_std]
#![forbid(unsafe_code)]
//! x86_64 register vocabulary. These values are Mosaic IDs, not Unicorn IDs.

use mosaic_core_types::RegId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Register {
    Rax,
    Rbx,
    Rcx,
    Rdx,
    Rsi,
    Rdi,
    Rsp,
    Rbp,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
    Rip,
}

impl From<Register> for RegId {
    fn from(reg: Register) -> Self {
        Self(reg as u32)
    }
}
