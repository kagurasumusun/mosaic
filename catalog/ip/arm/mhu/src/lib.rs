#![no_std]
#![forbid(unsafe_code)]

//! Arm CoreLink MHU（Message Handling Unit）ドアベル・メールボックスの
//! 機能モデル（catalog/ip/arm/mhu）。Juno 開発プラットフォームの MHU に
//! 倣い、3 つのドアベル・バンク（低優先度 NS / 高優先度 NS / セキュア）を
//! 持ちます。レジスタ地図は STAT/SET/CLEAR の 3 種類がバンクごとに
//! 0x100 間隔で並びます（Linux drivers/firmware/arm_mhu.c と同じ
//! オフセット: INTR_STAT 0x000 / INTR_SET 0x008 / INTR_CLEAR 0x00C）。
//!
//! 動作: 送信側が SET に書いたビットがバンクのドアベル状態に OR され、
//! 状態が 0 から非 0 になると受信側の IRQ 線（SignalId = バンク番号）が
//! 上がります。受信側は STAT を読んでから CLEAR に同じビットを書き、
//! 状態が 0 に戻ると IRQ 線が下がります。レベル意味論なので、ビットが
//! 残っている間の追加 CLEAR では線は動きません（設計書 §13.3 irq）。
//!
//! 他モデルへの参照は持たず、MMIO 回路と Ctx 経由でのみ外部と働きかけます
//! （設計書 §10, §12.4: コア間連携のメールボックスは runtime の責務）。

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId};

mod reg {
    pub const STAT: u64 = 0x000;
    pub const SET: u64 = 0x008;
    pub const CLEAR: u64 = 0x00C;
    pub const BANK_STRIDE: u64 = 0x100;
    pub const BANKS: u64 = 3;
}

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct MhuParams {
    pub unmapped: UnmappedPolicy,
}

impl MhuParams {
    pub const fn new(unmapped: UnmappedPolicy) -> Self {
        Self { unmapped }
    }
}

/// 不明オフセットへのアクセス時のポリシー（pl011/sp804 などと同じ作法）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored（予約領域の既定）。
    RazWi,
    /// ゲストに見えるデバイス障害。
    Fault,
}

/// スナップショット対象のデバイス状態（設計書 §10 state.rs に相当）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MhuState {
    pub schema: u32,
    /// バンクごとのドアベル状態ビット。
    pub stat: [u32; reg::BANKS as usize],
}

impl Default for MhuState {
    fn default() -> Self {
        Self {
            schema: 1,
            stat: [0; reg::BANKS as usize],
        }
    }
}

/// MHU ドアベル・メールボックス。
pub struct Mhu {
    params: MhuParams,
    state: MhuState,
}

impl Mhu {
    pub fn new(params: MhuParams) -> Self {
        Self {
            params,
            state: MhuState::default(),
        }
    }

    pub fn state(&self) -> MhuState {
        self.state
    }

    fn unmapped_read(&self) -> Result<u64, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }

    fn unmapped_write(&self) -> Result<(), BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(()),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }

    /// SET 書き込み: ビットを OR し、0 → 非 0 の遷移で受信側 IRQ を上げる。
    fn set(&mut self, ctx: &mut dyn Ctx, bank: usize, value: u32) -> Result<(), BusFault> {
        let before = self.state.stat[bank];
        self.state.stat[bank] |= value;
        if before == 0 && self.state.stat[bank] != 0 {
            ctx.set_signal(SignalId(bank as u32), true)
                .map_err(|_| BusFault::Device)?;
        }
        Ok(())
    }

    /// CLEAR 書き込み: ビットを落とし、非 0 → 0 の遷移で IRQ を下げる。
    fn clear(&mut self, ctx: &mut dyn Ctx, bank: usize, value: u32) -> Result<(), BusFault> {
        let before = self.state.stat[bank];
        self.state.stat[bank] &= !value;
        if before != 0 && self.state.stat[bank] == 0 {
            ctx.set_signal(SignalId(bank as u32), false)
                .map_err(|_| BusFault::Device)?;
        }
        Ok(())
    }
}

impl MmioTarget for Mhu {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        // ドアベル・レジスタは 32 ビット固定（arm_mhu.c は readl/writel のみ）。
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        if offset >= reg::BANK_STRIDE * reg::BANKS {
            return self.unmapped_read();
        }
        let bank = (offset / reg::BANK_STRIDE) as usize;
        match offset % reg::BANK_STRIDE {
            reg::STAT => Ok(u64::from(self.state.stat[bank])),
            // SET/CLEAR は書き込み専用。読み出しはポリシーに従う。
            _ => self.unmapped_read(),
        }
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        if offset >= reg::BANK_STRIDE * reg::BANKS {
            return self.unmapped_write();
        }
        let bank = (offset / reg::BANK_STRIDE) as usize;
        // ゲストが何を書いても panic しない: 下位 32 ビットだけを採用する。
        let bits = (value & 0xFFFF_FFFF) as u32;
        match offset % reg::BANK_STRIDE {
            reg::SET => self.set(ctx, bank, bits),
            reg::CLEAR => self.clear(ctx, bank, bits),
            // STAT は読み出し専用。書き込みは無視（ハードウェアと同等）。
            _ => self.unmapped_write(),
        }
    }
}

impl EventTarget for Mhu {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        // MHU はタイマを持たない。
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::sync::{Arc, Mutex};
    use std::vec;

    use super::*;
    use mosaic_core_types::{AccessKind, Endian, NodeId, Security, VTime};

    /// Ctx 実装: set_signal だけを記録する（他は未使用）。
    #[derive(Clone, Default)]
    struct SignalRecorder {
        edges: Arc<Mutex<Vec<(u32, bool)>>>,
    }

    impl Ctx for SignalRecorder {
        fn now(&self) -> VTime {
            VTime::ZERO
        }

        fn schedule(
            &mut self,
            _at: VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }

        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }

        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.edges.lock().unwrap().push((signal.0, high));
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: false,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: Endian::Little,
        }
    }

    fn mhu() -> (Mhu, SignalRecorder) {
        let recorder = SignalRecorder::default();
        (Mhu::new(MhuParams::new(UnmappedPolicy::RazWi)), recorder)
    }

    fn edges(recorder: &SignalRecorder) -> Vec<(u32, bool)> {
        recorder.edges.lock().unwrap().clone()
    }

    const BANK0_STAT: u64 = 0x000;
    const BANK0_SET: u64 = 0x008;
    const BANK0_CLEAR: u64 = 0x00C;
    const BANK1_STAT: u64 = 0x100;
    const BANK1_SET: u64 = 0x108;
    const BANK2_STAT: u64 = 0x200;
    const BANK2_SET: u64 = 0x208;
    const BANK2_CLEAR: u64 = 0x20C;

    #[test]
    fn set_stat_clear_cycle_drives_one_irq_edge_pair() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        mhu.write(&mut ctx, BANK0_SET, AccessSize::Word, 1, &attr)
            .unwrap();
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr),
            Ok(1)
        );
        assert_eq!(edges(&ctx), vec![(0, true)]);
        // STAT の読み出しに副作用はない。
        mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr)
            .unwrap();
        assert_eq!(edges(&ctx), vec![(0, true)]);
        mhu.write(&mut ctx, BANK0_CLEAR, AccessSize::Word, 1, &attr)
            .unwrap();
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr),
            Ok(0)
        );
        assert_eq!(edges(&ctx), vec![(0, true), (0, false)]);
    }

    #[test]
    fn partial_clear_keeps_the_irq_asserted_until_stat_reaches_zero() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        mhu.write(&mut ctx, BANK1_SET, AccessSize::Word, 0b11, &attr)
            .unwrap();
        mhu.write(&mut ctx, 0x10C, AccessSize::Word, 0b01, &attr)
            .unwrap();
        assert_eq!(
            mhu.read(&mut ctx, BANK1_STAT, AccessSize::Word, &attr),
            Ok(0b10)
        );
        // 立ち上がり 1 本だけ: ビットが残っている間は線は動かない。
        assert_eq!(edges(&ctx), vec![(1, true)]);
        mhu.write(&mut ctx, 0x10C, AccessSize::Word, 0b10, &attr)
            .unwrap();
        assert_eq!(edges(&ctx), vec![(1, true), (1, false)]);
    }

    #[test]
    fn banks_drive_independent_signals() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        mhu.write(&mut ctx, BANK2_SET, AccessSize::Word, 1, &attr)
            .unwrap();
        mhu.write(&mut ctx, BANK0_SET, AccessSize::Word, 1, &attr)
            .unwrap();
        assert_eq!(edges(&ctx), vec![(2, true), (0, true)]);
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr),
            Ok(1)
        );
        assert_eq!(
            mhu.read(&mut ctx, BANK1_STAT, AccessSize::Word, &attr),
            Ok(0)
        );
        assert_eq!(
            mhu.read(&mut ctx, BANK2_STAT, AccessSize::Word, &attr),
            Ok(1)
        );
        mhu.write(&mut ctx, BANK2_CLEAR, AccessSize::Word, 1, &attr)
            .unwrap();
        assert_eq!(edges(&ctx), vec![(2, true), (0, true), (2, false)]);
        assert_eq!(mhu.state().stat, [1, 0, 0]);
    }

    #[test]
    fn repeated_set_ors_bits_without_extra_edges() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        mhu.write(&mut ctx, BANK0_SET, AccessSize::Word, 1, &attr)
            .unwrap();
        mhu.write(&mut ctx, BANK0_SET, AccessSize::Word, 2, &attr)
            .unwrap();
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr),
            Ok(3)
        );
        assert_eq!(edges(&ctx), vec![(0, true)]);
    }

    #[test]
    fn non_word_sizes_are_rejected_and_stat_writes_are_ignored() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Byte, &attr),
            Err(BusFault::InvalidSize)
        );
        assert_eq!(
            mhu.write(&mut ctx, BANK0_SET, AccessSize::HalfWord, 1, &attr),
            Err(BusFault::InvalidSize)
        );
        // STAT への書き込みは無視され、ドアベル状態も IRQ も動かさない。
        mhu.write(&mut ctx, BANK0_STAT, AccessSize::Word, 0xFF, &attr)
            .unwrap();
        assert_eq!(
            mhu.read(&mut ctx, BANK0_STAT, AccessSize::Word, &attr),
            Ok(0)
        );
        assert!(edges(&ctx).is_empty());
    }

    #[test]
    fn offsets_outside_the_banks_follow_the_unmapped_policy() {
        let (mut mhu, mut ctx) = mhu();
        let attr = attr();
        assert_eq!(mhu.read(&mut ctx, 0x300, AccessSize::Word, &attr), Ok(0));
        mhu.write(&mut ctx, 0xFFC, AccessSize::Word, 1, &attr)
            .unwrap();
        let mut strict = Mhu::new(MhuParams::new(UnmappedPolicy::Fault));
        assert_eq!(
            strict.read(&mut ctx, 0x300, AccessSize::Word, &attr),
            Err(BusFault::Device)
        );
        assert_eq!(
            strict.write(&mut ctx, 0x300, AccessSize::Word, 1, &attr),
            Err(BusFault::Device)
        );
        // 64 ビット転送の値は下位 32 ビットだけが採用される。
        strict
            .write(&mut ctx, BANK0_SET, AccessSize::Word, 0x1_0000_0001, &attr)
            .unwrap();
        assert_eq!(strict.state().stat, [1, 0, 0]);
    }
}
