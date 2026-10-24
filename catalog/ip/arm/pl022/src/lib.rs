#![no_std]
#![forbid(unsafe_code)]
//! ARM PrimeCell 同期シリアルポート PL022 (SSP) の機能レジスタモデル
//! （構成設計書 P2 / ADR 0008、ARM DDI 0194）。
//!
//! ゲストは実機と同じ作法で駆動する: `SSPCR0` で 8 ビット・Motorola SPI
//! フレームと CPOL/CPHA を選び、`SSPCR1.SSE` を立てると CS がアサートされ、
//! `SSPDR` への 1 書き込みが 1 バイトの全二重交換になる。受信バイトは
//! RX FIFO に積まれ `SSPSR.RNE` 経由で観測、`SSPDR` 読み出しで取り出す。
//! `SSE` を落とすと CS が解放され、ターゲット側のフレーム状態もリセット
//! される（[`SpiInitiator::select`] `false` の契約経由）。
//!
//! 機能モデルの簡略化（タイミングを持たない宣言的モデル）:
//! - 転送は同期的に完了する（BSY は常に 0、TX FIFO は常に空）。
//! - マスタモード（`SSPCR1.MS = 0`）のみ modeled。スレーブモードで SSE を
//!   立てても CS は出ず、DR 書き込みは無視する。
//! - ループバック（`SSPCR1.LBM`）はバスへ出さず送信バイトを RX FIFO へ
//!   戻す。
//! - データビット数は DSS+1 ビットで MOSI/MISO を切り詰める。8 ビットを
//!   超えるフレームは下位 8 ビットだけ modeled する。
//! - 受信割り込み (RXRIS) は「RX FIFO 非空」で立つ（実機は半分以上）。
//! - 送信割り込み (TXRIS) は TX FIFO が常に空なので常に立つ（実機と同じ
//!   レベル条件の帰結）。
//! - 交換失敗（未配線・NoTarget など）と受信オーバーランは RORRIS で報告。

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_contract_spi::{SpiCs, SpiError, SpiInitiator, SpiMode};
use mosaic_core_types::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError};
use mosaic_ip_arm_primecell_id::PrimeCellId;

/// PL022 レジスタオフセット（DDI 0194 Table 3-1）。
mod reg {
    pub const CR0: u64 = 0x000;
    pub const CR1: u64 = 0x004;
    pub const DR: u64 = 0x008;
    pub const SR: u64 = 0x00C;
    pub const CPSR: u64 = 0x010;
    pub const IMSC: u64 = 0x014;
    pub const RIS: u64 = 0x018;
    pub const MIS: u64 = 0x01C;
    pub const ICR: u64 = 0x020;
    pub const DMACR: u64 = 0x024;
}

/// SSPCR0 ビット（FRF は Motorola SPI のみ modeled、SCR は保存のみ）。
mod cr0 {
    pub const DSS_MASK: u32 = 0xF;
    pub const CPHA: u32 = 1 << 6;
    pub const CPOL: u32 = 1 << 7;
}

/// SSPCR1 ビット（SOD はスレーブ専用なので modeled しない）。
mod cr1 {
    pub const LBM: u32 = 1 << 0;
    pub const SSE: u32 = 1 << 1;
    pub const MS: u32 = 1 << 2;
}

/// SSPSR ビット（BSY は同期実行のため常に 0）。
mod sr {
    pub const TFE: u32 = 1 << 0;
    pub const TNF: u32 = 1 << 1;
    pub const RNE: u32 = 1 << 2;
    pub const RFF: u32 = 1 << 3;
}

/// 割り込みビット（IMSC / RIS / MIS 共通配置）。
mod ris {
    pub const ROR: u32 = 1 << 0;
    pub const RT: u32 = 1 << 1;
    pub const RX: u32 = 1 << 2;
    pub const TX: u32 = 1 << 3;
}

/// RX/TX FIFO 深（DDI 0194: 8 エントリ）。
const FIFO_DEPTH: usize = 8;

/// 未マップオフセットの扱い。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored（既定）。
    RazWi,
    /// ゲスト可視のデバイスフォルト。
    Fault,
}

/// 構築パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct Pl022Params {
    /// combined 割り込み線。
    pub combined_irq: SignalId,
    /// この SSP が駆動するチップセレクト線。
    pub cs: SpiCs,
    /// 未マップオフセットの扱い。
    pub unmapped: UnmappedPolicy,
}

impl Pl022Params {
    pub const fn new(combined_irq: SignalId) -> Self {
        Self {
            combined_irq,
            cs: SpiCs(0),
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// スナップショット対象のスカラーレジスタ群（設計書 §10 の state 分離）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pl022State {
    pub schema: u32,
    pub cr0: u32,
    pub cr1: u32,
    pub cpsr: u32,
    pub imsc: u32,
    pub raw_interrupts: u32,
    pub dmacr: u32,
    pub irq_level: bool,
}

impl Default for Pl022State {
    fn default() -> Self {
        Self {
            schema: 1,
            cr0: 0,
            cr1: 0,
            cpsr: 0,
            imsc: 0,
            // TX FIFO は常に空なので TXRIS はリセット直後から立つ（実機同じ）。
            raw_interrupts: ris::TX,
            dmacr: 0,
            irq_level: false,
        }
    }
}

/// 機能 PL022。Initiator ポートは構築時に注入する（ADR 0008）。
pub struct Pl022 {
    params: Pl022Params,
    state: Pl022State,
    rx: VecDeque<u8>,
    identity: PrimeCellId,
    initiator: Option<Box<dyn SpiInitiator>>,
    /// Last chip-select level accepted by the interconnect. Kept separately
    /// from CR1 so a rejected select can be retried by rewriting CR1.
    cs_asserted: bool,
}

impl Pl022 {
    /// PL022 part number 0x022、revision 0、patch 0。
    pub const PART_NUMBER: u16 = 0x022;

    pub fn new(params: Pl022Params, initiator: Option<Box<dyn SpiInitiator>>) -> Self {
        Self {
            params,
            state: Pl022State::default(),
            rx: VecDeque::new(),
            identity: PrimeCellId::arm(Self::PART_NUMBER, 0, 0)
                .expect("PL022 part number fits the ID fields"),
            initiator,
            cs_asserted: false,
        }
    }

    fn enabled(&self) -> bool {
        self.state.cr1 & cr1::SSE != 0
    }

    fn master(&self) -> bool {
        self.state.cr1 & cr1::MS == 0
    }

    fn mode(&self) -> SpiMode {
        SpiMode::new(
            self.state.cr0 & cr0::CPOL != 0,
            self.state.cr0 & cr0::CPHA != 0,
        )
    }

    fn status(&self) -> u32 {
        let mut value = sr::TFE | sr::TNF;
        if !self.rx.is_empty() {
            value |= sr::RNE;
        }
        if self.rx.len() >= FIFO_DEPTH {
            value |= sr::RFF;
        }
        value
    }

    fn push_rx(&mut self, byte: u8) {
        if self.rx.len() >= FIFO_DEPTH {
            // 実機の受信オーバーラン: 満杯時の新着は失われ RORRIS が立つ。
            self.state.raw_interrupts |= ris::ROR;
            return;
        }
        self.rx.push_back(byte);
        self.refresh_level_bits();
    }

    fn refresh_level_bits(&mut self) {
        if self.rx.is_empty() {
            self.state.raw_interrupts &= !ris::RX;
        } else {
            self.state.raw_interrupts |= ris::RX;
        }
        // TX FIFO は常に空: TXRIS は常に立つ。
        self.state.raw_interrupts |= ris::TX;
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let asserted = self.state.raw_interrupts & self.state.imsc != 0;
        if asserted != self.state.irq_level {
            set_optional_signal(ctx, self.params.combined_irq, asserted)?;
            self.state.irq_level = asserted;
        }
        Ok(())
    }

    /// SSE の 0→1 で CS をアサート、1→0 で解放する（マスタモードのみ）。
    fn apply_cr1(&mut self, ctx: &mut dyn Ctx, value: u32) -> Result<(), BusFault> {
        self.state.cr1 = value & 0xF;
        let desired = self.enabled() && self.master();
        if desired == self.cs_asserted {
            return Ok(());
        }
        if let Some(initiator) = self.initiator.as_mut() {
            // NoTarget is an optional wiring choice. Other failures leave the
            // last accepted level untouched so a later CR1 write retries it.
            match initiator.select(ctx, self.params.cs, desired) {
                Ok(()) => {
                    self.cs_asserted = desired;
                    Ok(())
                }
                Err(SpiError::NoTarget) => {
                    // An absent target is an optional composition choice, but
                    // it is not an acknowledged assertion. Keep the cached
                    // level low so a later CR1 write can retry after wiring is
                    // completed. Deassertion remains safe without a target.
                    if !desired {
                        self.cs_asserted = false;
                    }
                    Ok(())
                }
                Err(_) => Err(BusFault::Device),
            }
        } else {
            // No injected port means SPI output is intentionally unconnected.
            self.cs_asserted = desired;
            Ok(())
        }
    }

    /// DSS+1 ビットのフレームマスク（8 ビット超は下位 8 ビット）。
    fn frame_mask(&self) -> u8 {
        let bits = (self.state.cr0 & cr0::DSS_MASK) + 1;
        if bits >= 8 {
            0xFF
        } else {
            ((1u32 << bits) - 1) as u8
        }
    }

    fn exchange(&mut self, ctx: &mut dyn Ctx, byte: u8) {
        let mask = self.frame_mask();
        if self.state.cr1 & cr1::LBM != 0 {
            // ループバック: 送信バイトがそのまま RX FIFO へ戻る。
            self.push_rx(byte & mask);
            return;
        }
        // 借用分割のため、モードと CS を initiator の可変借用より先に確定。
        let mode = self.mode();
        let cs = self.params.cs;
        let Some(initiator) = self.initiator.as_mut() else {
            return;
        };
        let mosi = [byte & mask];
        let mut miso = [0u8; 1];
        match initiator.exchange(ctx, cs, mode, &mosi, &mut miso) {
            Ok(()) => self.push_rx(miso[0] & mask),
            // 交換失敗はゲスト可視のエラーステータスとして RORRIS を使う
            //（機能モデルの簡略化。実機の MODEFAULT 等は modeled しない）。
            Err(SpiError::Overflow) => {
                self.state.raw_interrupts |= ris::ROR;
                self.push_rx(miso[0] & mask);
            }
            Err(_) => self.state.raw_interrupts |= ris::ROR,
        }
    }

    fn read_word(&mut self, offset: u64) -> Result<u32, BusFault> {
        let value = match offset {
            reg::CR0 => self.state.cr0,
            reg::CR1 => self.state.cr1,
            reg::DR => {
                let byte = self.rx.pop_front().unwrap_or(0);
                self.refresh_level_bits();
                u32::from(byte)
            }
            reg::SR => self.status(),
            reg::CPSR => self.state.cpsr,
            reg::IMSC => self.state.imsc,
            reg::RIS => self.state.raw_interrupts,
            reg::MIS => self.state.raw_interrupts & self.state.imsc,
            reg::ICR => 0,
            reg::DMACR => self.state.dmacr,
            _ => {
                if PrimeCellId::covers(offset) {
                    self.identity.read_word(offset).unwrap_or(0)
                } else {
                    return self.unmapped(offset);
                }
            }
        };
        Ok(value)
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u32) -> Result<(), BusFault> {
        let mut operation_result = Ok(());
        match offset {
            reg::CR0 => self.state.cr0 = value & 0xFFFF,
            reg::CR1 => operation_result = self.apply_cr1(ctx, value),
            reg::DR => {
                // 無効化中・スレーブモードの DR 書き込みは modeled しない。
                if self.enabled() && self.master() {
                    self.exchange(ctx, (value & 0xFF) as u8);
                }
            }
            reg::CPSR => self.state.cpsr = value & 0xFF,
            reg::IMSC => self.state.imsc = value & 0xF,
            reg::ICR => {
                // write-1-clear: RORIC / RTIC のみ。
                self.state.raw_interrupts &= !(value & (ris::ROR | ris::RT));
            }
            reg::DMACR => self.state.dmacr = value & 0x3,
            // 読み出し専用レジスタへの書き込みは無視。
            reg::SR | reg::RIS | reg::MIS => {}
            _ => {
                if PrimeCellId::covers(offset) {
                    // 識別レジスタは読み出し専用。
                } else {
                    return self.unmapped(offset).map(|_| ());
                }
            }
        }
        let irq_result = self.update_irq(ctx).map_err(|_| BusFault::Device);
        operation_result.and(irq_result)
    }

    fn unmapped(&self, _offset: u64) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Pl022 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            // These PrimeCell register banks are 32-bit wide; reject before
            // performing FIFO/interrupt side effects of the read.
            return Err(BusFault::InvalidSize);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(offset)?;
        // DR 読み出しは RX FIFO のレベルビットと割り込み線を変える。
        self.update_irq(ctx).map_err(|_| BusFault::Device)?;
        Ok(u64::from(word & mask))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            // PL022 レジスタは 32 ビット幅。8 バイト転送は収まらない。
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for Pl022 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// Emulated PL022 checkpoint. The SPI initiator peer is a host port.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pl022Checkpoint {
    pub schema: u32,
    pub registers: Pl022State,
    pub rx: Vec<u8>,
    pub cs_asserted: bool,
}

impl Snapshot for Pl022 {
    type State = Pl022Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Pl022Checkpoint {
            schema: Self::SCHEMA_VERSION,
            registers: self.state,
            rx: self.rx.iter().copied().collect(),
            cs_asserted: self.cs_asserted,
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION
            || state.schema != Self::SCHEMA_VERSION
            || state.registers.schema != 1
        {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.rx.len() > FIFO_DEPTH {
            return Err(SnapshotError::InvalidState);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.registers;
        self.rx = state.rx.iter().copied().collect();
        self.cs_asserted = state.cs_asserted;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.u32(state.registers.cr0);
        hash.u32(state.registers.cr1);
        hash.u32(state.registers.cpsr);
        hash.u32(state.registers.imsc);
        hash.u32(state.registers.raw_interrupts);
        hash.u32(state.registers.dmacr);
        hash.bool(state.registers.irq_level);
        hash.bytes(&state.rx);
        hash.bool(state.cs_asserted);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::sync::{Arc, Mutex};
    use std::vec;

    use super::*;
    use mosaic_core_types::{AccessKind, Endian, EventId, NodeId, Security, VTime};

    const IRQ: SignalId = SignalId(11);

    #[derive(Default)]
    struct Fake {
        selects: Vec<(SpiCs, bool)>,
        exchanges: Vec<(SpiCs, SpiMode, Vec<u8>)>,
        miso_fill: u8,
        fail: Option<SpiError>,
        fail_select: Option<SpiError>,
    }

    struct FakeHandle(Arc<Mutex<Fake>>);

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl SpiInitiator for FakeHandle {
        fn select(&mut self, _ctx: &mut dyn Ctx, cs: SpiCs, assert: bool) -> Result<(), SpiError> {
            let mut fake = lock(&self.0);
            if let Some(error) = fake.fail_select {
                return Err(error);
            }
            fake.selects.push((cs, assert));
            Ok(())
        }
        fn exchange(
            &mut self,
            _ctx: &mut dyn Ctx,
            cs: SpiCs,
            mode: SpiMode,
            mosi: &[u8],
            miso: &mut [u8],
        ) -> Result<(), SpiError> {
            let mut fake = lock(&self.0);
            fake.exchanges.push((cs, mode, mosi.to_vec()));
            if let Some(error) = fake.fail {
                return Err(error);
            }
            let mut fill = fake.miso_fill;
            for slot in miso.iter_mut() {
                *slot = fill;
                fill = fill.wrapping_add(1);
            }
            fake.miso_fill = fill;
            Ok(())
        }
    }

    struct TestCtx {
        signals: Vec<(SignalId, bool)>,
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime::ZERO
        }
        fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
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

    fn device() -> (Pl022, Arc<Mutex<Fake>>, TestCtx) {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let device = Pl022::new(
            Pl022Params::new(IRQ),
            Some(Box::new(FakeHandle(Arc::clone(&fake)))),
        );
        (
            device,
            fake,
            TestCtx {
                signals: Vec::new(),
            },
        )
    }

    fn read(device: &mut Pl022, ctx: &mut TestCtx, offset: u64) -> u32 {
        device.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn write(device: &mut Pl022, ctx: &mut TestCtx, offset: u64, value: u32) {
        device
            .write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn primecell_identification_reads_back() {
        let (mut device, _, mut ctx) = device();
        assert_eq!(read(&mut device, &mut ctx, 0xFE0), 0x22);
        // PID1 = part[11:8] | JEP104 identity[3:0] << 4.
        assert_eq!(read(&mut device, &mut ctx, 0xFE4), 0x10);
        assert_eq!(read(&mut device, &mut ctx, 0xFF0), 0x0D);
        assert_eq!(read(&mut device, &mut ctx, 0xFF4), 0xF0);
        assert_eq!(read(&mut device, &mut ctx, 0xFF8), 0x05);
        assert_eq!(read(&mut device, &mut ctx, 0xFFC), 0xB1);
    }

    #[test]
    fn por_status_has_empty_fifos_and_txris_set() {
        let (mut device, _, mut ctx) = device();
        assert_eq!(read(&mut device, &mut ctx, reg::SR), sr::TFE | sr::TNF);
        assert_eq!(read(&mut device, &mut ctx, reg::RIS), ris::TX);
        assert_eq!(read(&mut device, &mut ctx, reg::MIS), 0);
    }

    #[test]
    fn sse_edges_drive_chip_select() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        assert_eq!(lock(&fake).selects, vec![(SpiCs(0), true)]);
        // 同じ値の再書き込みはエッジではない。
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        assert_eq!(lock(&fake).selects.len(), 1);
        write(&mut device, &mut ctx, reg::CR1, 0);
        assert_eq!(
            lock(&fake).selects,
            vec![(SpiCs(0), true), (SpiCs(0), false)]
        );
    }

    #[test]
    fn missing_chip_select_does_not_cache_an_unacknowledged_assertion() {
        let (mut device, fake, mut ctx) = device();
        lock(&fake).fail_select = Some(SpiError::NoTarget);

        // An unconnected target is allowed, but the local cache must not claim
        // CS was asserted when the interconnect did not accept the request.
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        assert!(lock(&fake).selects.is_empty());

        lock(&fake).fail_select = None;
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        assert_eq!(lock(&fake).selects, vec![(SpiCs(0), true)]);
    }

    #[test]
    fn rejected_chip_select_can_be_retried_without_an_extra_cr1_edge() {
        let (mut device, fake, mut ctx) = device();
        lock(&fake).fail_select = Some(SpiError::ModeFault);
        assert_eq!(
            device.write(
                &mut ctx,
                reg::CR1,
                AccessSize::Word,
                u64::from(cr1::SSE),
                &attr()
            ),
            Err(BusFault::Device)
        );
        assert!(lock(&fake).selects.is_empty());

        lock(&fake).fail_select = None;
        // CR1 has the same value, but CS was never successfully asserted.
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        assert_eq!(lock(&fake).selects, vec![(SpiCs(0), true)]);
    }

    #[test]
    fn dr_write_exchanges_one_byte_and_dr_read_pops_it() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::CR0, 0x07); // DSS = 8 bit
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        lock(&fake).miso_fill = 0x60;
        write(&mut device, &mut ctx, reg::DR, 0xD0);
        assert_eq!(
            lock(&fake).exchanges,
            vec![(SpiCs(0), SpiMode::new(false, false), vec![0xD0])]
        );
        assert_ne!(read(&mut device, &mut ctx, reg::SR) & sr::RNE, 0);
        assert_eq!(read(&mut device, &mut ctx, reg::DR), 0x60);
        assert_eq!(read(&mut device, &mut ctx, reg::SR) & sr::RNE, 0);
    }

    #[test]
    fn cr0_cpol_cpha_reach_the_exchange_mode() {
        let (mut device, fake, mut ctx) = device();
        write(
            &mut device,
            &mut ctx,
            reg::CR0,
            0x07 | cr0::CPOL | cr0::CPHA,
        );
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        write(&mut device, &mut ctx, reg::DR, 0x01);
        assert_eq!(
            lock(&fake).exchanges,
            vec![(SpiCs(0), SpiMode::new(true, true), vec![0x01])]
        );
    }

    #[test]
    fn loopback_returns_the_byte_without_touching_the_bus() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::CR0, 0x07);
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE | cr1::LBM);
        write(&mut device, &mut ctx, reg::DR, 0x5A);
        assert!(lock(&fake).exchanges.is_empty());
        assert_ne!(read(&mut device, &mut ctx, reg::SR) & sr::RNE, 0);
        assert_eq!(read(&mut device, &mut ctx, reg::DR), 0x5A);
    }

    #[test]
    fn dss_truncates_the_frame_width() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::CR0, 0x03); // DSS = 4 bit
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        lock(&fake).miso_fill = 0x6F;
        write(&mut device, &mut ctx, reg::DR, 0xFF);
        assert_eq!(
            lock(&fake).exchanges,
            vec![(SpiCs(0), SpiMode::new(false, false), vec![0x0F])]
        );
        // MISO も 4 ビットへ切り詰められる (0x6F → 0x0F)。
        assert_eq!(read(&mut device, &mut ctx, reg::DR), 0x0F);
    }

    #[test]
    fn disabled_or_slave_writes_do_not_exchange() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::DR, 0xAA);
        assert!(lock(&fake).exchanges.is_empty());
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE | cr1::MS);
        assert!(lock(&fake).selects.is_empty());
        write(&mut device, &mut ctx, reg::DR, 0xAA);
        assert!(lock(&fake).exchanges.is_empty());
    }

    #[test]
    fn rx_overrun_sets_and_icr_clears_rorris() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        for _ in 0..FIFO_DEPTH + 1 {
            write(&mut device, &mut ctx, reg::DR, 0x00);
        }
        assert_ne!(read(&mut device, &mut ctx, reg::SR) & sr::RFF, 0);
        assert_ne!(read(&mut device, &mut ctx, reg::RIS) & ris::ROR, 0);
        write(&mut device, &mut ctx, reg::ICR, ris::ROR);
        assert_eq!(read(&mut device, &mut ctx, reg::RIS) & ris::ROR, 0);
    }

    #[test]
    fn exchange_failure_reports_rorris() {
        let (mut device, fake, mut ctx) = device();
        lock(&fake).fail = Some(SpiError::NoTarget);
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        write(&mut device, &mut ctx, reg::DR, 0x00);
        assert_ne!(read(&mut device, &mut ctx, reg::RIS) & ris::ROR, 0);
        assert_eq!(read(&mut device, &mut ctx, reg::SR) & sr::RNE, 0);
    }

    #[test]
    fn irq_line_follows_masked_rx_status() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::IMSC, ris::RX);
        assert!(ctx.signals.is_empty());
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        write(&mut device, &mut ctx, reg::DR, 0x00);
        assert_eq!(ctx.signals, vec![(IRQ, true)]);
        // DR 読み出しで FIFO が空になると線が落ちる。
        read(&mut device, &mut ctx, reg::DR);
        assert_eq!(ctx.signals, vec![(IRQ, true), (IRQ, false)]);
    }

    #[test]
    fn unmapped_and_oversized_accesses_follow_policy() {
        let (mut device, _, mut ctx) = device();
        assert_eq!(read(&mut device, &mut ctx, 0x100), 0);
        write(&mut device, &mut ctx, 0x100, 1);
        assert_eq!(
            device.write(&mut ctx, reg::DR, AccessSize::DoubleWord, 0, &attr()),
            Err(BusFault::InvalidSize)
        );
        let mut strict = Pl022Params::new(IRQ);
        strict.unmapped = UnmappedPolicy::Fault;
        let mut strict_device = Pl022::new(strict, None);
        assert_eq!(
            strict_device.read(&mut ctx, 0x100, AccessSize::Word, &attr()),
            Err(BusFault::Device)
        );
    }

    #[test]
    fn unwired_port_tolerates_dr_writes() {
        let mut device = Pl022::new(Pl022Params::new(IRQ), None);
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        write(&mut device, &mut ctx, reg::CR1, cr1::SSE);
        write(&mut device, &mut ctx, reg::DR, 0x55);
        assert_eq!(read(&mut device, &mut ctx, reg::SR) & sr::RNE, 0);
    }
}
