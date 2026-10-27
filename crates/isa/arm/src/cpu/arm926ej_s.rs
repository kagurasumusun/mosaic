//! ARM926EJ-S CPU コアモデル（isa/arm、QEMU の target/arm と同じ居場所）。
//! ARM DDI 0193E（ARM926EJ-S テクニカルリファレンスマニュアル）に基づく
//! クリーンルーム実装。ISA 語彙は mosaic-isa-arm、実行エンジンは
//! mosaic-engine-* に委ね、このモデルはコア固有レジスタ窓（MIDR、制御
//! レジスタ、ステータスレジスタ）を MMIO ターゲットとして提供するだけです。
//! 他モデルへの参照を持たず、MMIO 回路と Ctx 経由でのみ外部と働きかけます
//! （設計書 §10, §12.1）。

use mosaic_contract::cpu_engine::{CpuDescriptor, EngineArch};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::Ctx;
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, Snapshot, SnapshotError};

/// 実行構成の宣言（設計書 §12.1: cpu = ISA + エンジン設定 + コア固有部分）。
/// A32 リトルエンディアン。backend 既定の CPU モデルを使うため model は None。
pub const CORE: CpuDescriptor = CpuDescriptor::new(EngineArch::ArmA32Le, None);

/// このコアの ISA 語彙（RegId はここから引く）。
pub use crate::Register;

// ARM926EJ-S の Main ID Register: implementer 0x41 (Arm)、アーキテクチャ 0x5 (ARMv5TEJ)、
// パート番号 0x926、リビジョン 0x1。
const MIDR: u32 = 0x4105_9261;

mod reg {
    pub const MIDR: u64 = 0x000;
    pub const CONTROL: u64 = 0x004;
    pub const STATUS: u64 = 0x008;
}

mod control {
    pub const ENABLE: u32 = 1 << 0;
}

mod status {
    pub const IRQ_LEVEL: u32 = 1 << 0;
    pub const FIQ_LEVEL: u32 = 1 << 1;
}

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct Arm926ejSParams {
    pub unmapped: UnmappedPolicy,
}

impl Arm926ejSParams {
    pub const fn new(unmapped: UnmappedPolicy) -> Self {
        Self { unmapped }
    }
}

/// スナップショット対象のデバイス状態（設計書 §10 state.rs に相当）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arm926ejSState {
    pub schema: u32,
    pub control: u32,
    pub irq_level: bool,
    pub fiq_level: bool,
}

impl Default for Arm926ejSState {
    fn default() -> Self {
        Self {
            schema: 1,
            control: 0,
            irq_level: false,
            fiq_level: false,
        }
    }
}

/// 不明オフセットへのアクセス時のポリシー。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored（予約領域の既定）。
    RazWi,
    /// ゲストに見えるデバイス障害。
    Fault,
}

/// ARM926EJ-S コアモデル。IRQ/FIQ ラインのレベルは set_irq / set_fiq で
/// SoC 側の配線または glue から供給され、ステータスレジスタ経由で読み出せます。
pub struct Arm926ejS {
    state: Arm926ejSState,
    params: Arm926ejSParams,
}

impl Arm926ejS {
    pub fn new(params: Arm926ejSParams) -> Self {
        Self {
            state: Arm926ejSState::default(),
            params,
        }
    }

    /// IRQ ラインのレベルを供給する。
    pub fn set_irq(&mut self, high: bool) {
        self.state.irq_level = high;
    }

    /// FIQ ラインのレベルを供給する。
    pub fn set_fiq(&mut self, high: bool) {
        self.state.fiq_level = high;
    }

    fn unmapped_read(&self) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Arm926ejS {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if offset % 4 != 0 {
            return self.unmapped_read().map(u64::from);
        }
        let value = match offset {
            reg::MIDR => MIDR,
            reg::CONTROL => self.state.control,
            reg::STATUS => {
                let mut flags = 0u32;
                if self.state.irq_level {
                    flags |= status::IRQ_LEVEL;
                }
                if self.state.fiq_level {
                    flags |= status::FIQ_LEVEL;
                }
                flags
            }
            _ => return self.unmapped_read().map(u64::from),
        };
        Ok(match size {
            AccessSize::Byte => u64::from(value & 0xFF),
            AccessSize::HalfWord => u64::from(value & 0xFFFF),
            AccessSize::Word | AccessSize::DoubleWord => u64::from(value),
        })
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        _size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if offset % 4 != 0 {
            return self.unmapped_read().map(|_| ());
        }
        // レジスタ幅は 32 ビット。64 ビット転送の値は下位 32 ビットを採用する
        //（pl011/sp804 と同じくゲスト入力でパニックさせない）。
        let reg_value =
            u32::try_from(value & 0xFFFF_FFFF).expect("下位 32 ビットは常に u32 に収まる");
        match offset {
            reg::CONTROL => {
                self.state.control = reg_value & control::ENABLE;
                Ok(())
            }
            // 読み出し専用レジスタへの書き込みは無視（ハードウェアと同等）。
            reg::MIDR | reg::STATUS => Ok(()),
            _ => self.unmapped_read().map(|_| ()),
        }
    }
}

impl Snapshot for Arm926ejS {
    type State = Arm926ejSState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = *state;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.u32(state.control);
        hash.bool(state.irq_level);
        hash.bool(state.fiq_level);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Arm926ejSParams {
        Arm926ejSParams::new(UnmappedPolicy::RazWi)
    }

    fn attr() -> Attr {
        Attr {
            master: mosaic_exec::NodeId(0),
            security: mosaic_exec::Security::NonSecure,
            privileged: false,
            kind: mosaic_exec::AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: mosaic_exec::Endian::Little,
        }
    }

    struct CtxStub;

    impl Ctx for CtxStub {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }

        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }

        fn set_signal(
            &mut self,
            _signal: mosaic_exec::SignalId,
            _high: bool,
        ) -> Result<(), mosaic_contract::model::ContextError> {
            Err(mosaic_contract::model::ContextError::UnknownSignal)
        }
    }

    #[test]
    fn midr_identifies_arm926ej_s_and_is_read_only() {
        let mut core = Arm926ejS::new(params());
        let mut ctx = CtxStub;
        let attr = attr();
        assert_eq!(
            core.read(&mut ctx, reg::MIDR, AccessSize::Word, &attr),
            Ok(MIDR as u64)
        );
        core.write(&mut ctx, reg::MIDR, AccessSize::Word, 0xDEAD_BEEF, &attr)
            .unwrap();
        assert_eq!(
            core.read(&mut ctx, reg::MIDR, AccessSize::Word, &attr),
            Ok(MIDR as u64)
        );
    }

    #[test]
    fn control_register_is_read_write_and_masks_to_enable_bit() {
        let mut core = Arm926ejS::new(params());
        let mut ctx = CtxStub;
        let attr = attr();
        core.write(&mut ctx, reg::CONTROL, AccessSize::Word, 0xFFFF_FFFF, &attr)
            .unwrap();
        assert_eq!(
            core.read(&mut ctx, reg::CONTROL, AccessSize::Word, &attr),
            Ok(control::ENABLE as u64)
        );
    }

    #[test]
    fn irq_and_fiq_levels_are_reflected_in_status() {
        let mut core = Arm926ejS::new(params());
        let mut ctx = CtxStub;
        let attr = attr();
        core.set_irq(true);
        assert_eq!(
            core.read(&mut ctx, reg::STATUS, AccessSize::Word, &attr),
            Ok(status::IRQ_LEVEL as u64)
        );
        core.set_fiq(true);
        assert_eq!(
            core.read(&mut ctx, reg::STATUS, AccessSize::Word, &attr),
            Ok((status::IRQ_LEVEL | status::FIQ_LEVEL) as u64)
        );
        core.set_irq(false);
        assert_eq!(
            core.read(&mut ctx, reg::STATUS, AccessSize::Word, &attr),
            Ok(status::FIQ_LEVEL as u64)
        );
    }

    #[test]
    fn byte_and_halfword_reads_return_low_part() {
        let mut core = Arm926ejS::new(params());
        let mut ctx = CtxStub;
        let attr = attr();
        // CONTROL keeps only the ENABLE bit, so width masking is observed on
        // the full-width read-only MIDR (0x4105_9261).
        assert_eq!(
            core.read(&mut ctx, reg::MIDR, AccessSize::Byte, &attr),
            Ok(0x61)
        );
        assert_eq!(
            core.read(&mut ctx, reg::MIDR, AccessSize::HalfWord, &attr),
            Ok(0x9261)
        );
        assert_eq!(
            core.read(&mut ctx, reg::MIDR, AccessSize::Word, &attr),
            Ok(MIDR as u64)
        );
    }

    #[test]
    fn misaligned_offset_follows_unmapped_policy() {
        let mut faulting = Arm926ejS::new(Arm926ejSParams::new(UnmappedPolicy::Fault));
        let mut ctx = CtxStub;
        let attr = attr();
        assert_eq!(
            faulting.read(&mut ctx, 1, AccessSize::Byte, &attr),
            Err(BusFault::Device)
        );
        let mut raz = Arm926ejS::new(params());
        assert_eq!(raz.read(&mut ctx, 1, AccessSize::Byte, &attr), Ok(0));
    }

    #[test]
    fn unknown_offset_follows_unmapped_policy() {
        let mut faulting = Arm926ejS::new(Arm926ejSParams::new(UnmappedPolicy::Fault));
        let mut ctx = CtxStub;
        let attr = attr();
        assert_eq!(
            faulting.read(&mut ctx, 0x1000, AccessSize::Word, &attr),
            Err(BusFault::Device)
        );
        assert_eq!(
            faulting.write(&mut ctx, 0x1000, AccessSize::Word, 1, &attr),
            Err(BusFault::Device)
        );
        let mut raz = Arm926ejS::new(params());
        assert_eq!(raz.read(&mut ctx, 0x1000, AccessSize::Word, &attr), Ok(0));
        assert_eq!(
            raz.write(&mut ctx, 0x1000, AccessSize::Word, 1, &attr),
            Ok(())
        );
    }

    #[test]
    fn write_value_is_truncated_to_register_width() {
        let mut core = Arm926ejS::new(params());
        let mut ctx = CtxStub;
        let attr = attr();
        core.write(
            &mut ctx,
            reg::CONTROL,
            AccessSize::DoubleWord,
            0x1_0000_0001,
            &attr,
        )
        .unwrap();
        // 下位 32 ビットのうち制御レジスタは ENABLE ビットだけ保持する。
        assert_eq!(
            core.read(&mut ctx, reg::CONTROL, AccessSize::Word, &attr),
            Ok(control::ENABLE as u64)
        );
    }
}
