//! RISC-V RV32I レジスタ語彙（設計書 §12.1 の `isa` 層）。
//!
//! これらの値は Mosaic の RegId であり、Unicorn などの backend 固有 ID では
//! ありません。番号は整数レジスタ x0..x31 が 0..31、PC が 32。x0 は
//! ハードワイヤードゼロ（書き込みはアーキテクチャ上無視される）。
//! ABI 名は RISC-V psABI v1.0 によります。

use mosaic_exec::RegId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Register {
    X0,
    X1,
    X2,
    X3,
    X4,
    X5,
    X6,
    X7,
    X8,
    X9,
    X10,
    X11,
    X12,
    X13,
    X14,
    X15,
    X16,
    X17,
    X18,
    X19,
    X20,
    X21,
    X22,
    X23,
    X24,
    X25,
    X26,
    X27,
    X28,
    X29,
    X30,
    X31,
    Pc,
}

impl Register {
    // psABI v1.0 の呼び出し規約名。同じレジスタの別名です。
    pub const ZERO: Self = Self::X0;
    pub const RA: Self = Self::X1;
    pub const SP: Self = Self::X2;
    pub const GP: Self = Self::X3;
    pub const TP: Self = Self::X4;
    pub const T0: Self = Self::X5;
    pub const T1: Self = Self::X6;
    pub const T2: Self = Self::X7;
    pub const S0: Self = Self::X8;
    pub const FP: Self = Self::X8;
    pub const S1: Self = Self::X9;
    pub const A0: Self = Self::X10;
    pub const A1: Self = Self::X11;
    pub const A2: Self = Self::X12;
    pub const A3: Self = Self::X13;
    pub const A4: Self = Self::X14;
    pub const A5: Self = Self::X15;
    pub const A6: Self = Self::X16;
    pub const A7: Self = Self::X17;
    pub const S2: Self = Self::X18;
    pub const S3: Self = Self::X19;
    pub const S4: Self = Self::X20;
    pub const S5: Self = Self::X21;
    pub const S6: Self = Self::X22;
    pub const S7: Self = Self::X23;
    pub const S8: Self = Self::X24;
    pub const S9: Self = Self::X25;
    pub const S10: Self = Self::X26;
    pub const S11: Self = Self::X27;
    pub const T3: Self = Self::X28;
    pub const T4: Self = Self::X29;
    pub const T5: Self = Self::X30;
    pub const T6: Self = Self::X31;
}

impl From<Register> for RegId {
    fn from(reg: Register) -> Self {
        Self(reg as u32)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    #[test]
    fn register_ids_follow_the_x0_through_pc_order() {
        assert_eq!(RegId::from(Register::X0), RegId(0));
        assert_eq!(RegId::from(Register::X31), RegId(31));
        assert_eq!(RegId::from(Register::Pc), RegId(32));
    }

    #[test]
    fn abi_aliases_name_the_same_registers() {
        assert_eq!(Register::RA, Register::X1);
        assert_eq!(Register::SP, Register::X2);
        assert_eq!(Register::A0, Register::X10);
        assert_eq!(Register::S0, Register::FP);
        assert_eq!(Register::T6, Register::X31);
    }
}
