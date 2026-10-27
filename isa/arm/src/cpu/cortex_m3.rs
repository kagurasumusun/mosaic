//! ARM Cortex-M3 コア宣言（isa/arm、QEMU の target/arm と同じ居場所）。
//!
//! 設計書 §12.1 の cpu 層のうち、ISA 語彙とエンジン設定とコアの例外入力だけを
//! 持つ。NVIC・SysTick・SCB は統合ブロックではなく独立 IP `ip:arm/scs` であり、
//! SoC の宣言で結合する。CP15 や SCS を模した MMIO 窓は作らない。
//!
//! `model` は backend の CPU モデル選択ヒントである。Unicorn の M-class 構成は
//! ADR 0012、例外入力の注入能力は ADR 0009 が決める。このクレートは実行しない。

use mosaic_contract::cpu_engine::{CpuDescriptor, EngineArch, IrqLine};

/// 実行構成の宣言。ARMv7-M リトルエンディアン、モデルは cortex-m3。
pub const CORE: CpuDescriptor = CpuDescriptor::new(EngineArch::ArmV7mLe, Some("cortex-m3"));

/// コア自身のマスク可能例外入力。NVIC の外部 IRQ 番号ではない。
/// Unicorn M-class アダプタが受理するラインはこれだけである（ADR 0009 / 0012）。
pub const EXCEPTION_LINE: IrqLine = IrqLine(0);

/// このコアの ISA 語彙（RegId はここから引く）。
pub use crate::Register;

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    #[test]
    fn the_descriptor_names_armv7m_and_does_not_pretend_to_be_an_mmio_block() {
        assert_eq!(CORE.arch, EngineArch::ArmV7mLe);
        assert_eq!(CORE.model, Some("cortex-m3"));
        assert_eq!(EXCEPTION_LINE, IrqLine(0));
        assert_eq!(Register::Sp as u32, 13);
    }
}
