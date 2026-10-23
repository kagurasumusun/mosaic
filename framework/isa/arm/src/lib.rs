#![no_std]
#![forbid(unsafe_code)]
//! ARM A32 register vocabulary. These values are Mosaic IDs, not Unicorn IDs.

use mosaic_core_types::RegId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Register {
    R0,
    R1,
    R2,
    R3,
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    Sp,
    Lr,
    Pc,
}

impl From<Register> for RegId {
    fn from(reg: Register) -> Self {
        Self(reg as u32)
    }
}
