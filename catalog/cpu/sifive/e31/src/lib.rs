#![no_std]
#![forbid(unsafe_code)]

//! SiFive E31 コア宣言（catalog/cpu/sifive/e31）。
//!
//! E31 は RV32IMAC の順序実行コアで、Freedom E310（HiFive1）に載る
//! ものです（SiFive FE310-G000 マニュアル）。このクレートは設計書 §12.1 の
//! cpu 層のうち「ISA + エンジン設定」の宣言を持ち、P3 の異種 AMP
//! テストベンチでは RV32I 基本整数命令の部分集合を実行します。CLINT/PLIC
//! などの統合ブロックは独立 IP として catalog/ip に置き、SoC の宣言で
//! 結合します（このコア・モデルには含めません）。
//!
//! `model` は backend の CPU モデル選択ヒントです。Unicorn 2.1.x の Rust
//! バインディングは CPU モデル選択を公開していないため、現在は宣言のみで
//! backend 既定の RV32 コアが実行を担います（ADR 0010）。

use mosaic_contract_cpu_engine::{CpuDescriptor, EngineArch};

/// 実行構成の宣言。RV32 リトルエンディアン、モデルは sifive-e31。
pub const CORE: CpuDescriptor = CpuDescriptor::new(EngineArch::RiscV32Le, Some("sifive-e31"));

/// このコアの ISA 語彙（RegId はここから引く）。
pub use mosaic_isa_riscv::Register;

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    #[test]
    fn the_descriptor_declares_rv32_and_the_e31_model() {
        assert_eq!(CORE.arch, EngineArch::RiscV32Le);
        assert_eq!(CORE.model, Some("sifive-e31"));
        assert_eq!(Register::A0, Register::X10);
    }
}
