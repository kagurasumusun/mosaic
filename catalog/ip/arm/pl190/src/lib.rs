#![no_std]
#![forbid(unsafe_code)]
//! ARM PrimeCell ベクタ割り込みコントローラ PL190 の機能レジスタモデル
//! （構成設計書 P3 / ARM DDI 0181E、参照実装 QEMU hw/intc/pl190.c）。
//!
//! 32 本の割り込み入力を集約し、ベクタ優先度方式で 1 本の IRQ と 1 本の
//! FIQ を出力する。ゲストは実機と同じ作法で駆動する:
//!
//! - 入力ラインは [`EventTarget::on_signal`] で源識別付きで届く。配線は
//!   [`Pl190::bind_input`] で (source node, signal id) → ライン番号を割り
//!   当てる（構成設計書 §13.3: 複数ソース・名前付きピン）。未割当の源は
//!   任意ポート同様に無視され、ゲスト可視の故障にはならない。
//! - `INTENABLE` は write-1-to-set、`INTENCLEAR` は write-1-to-clear。
//! - `INTSELECT` のビットが 1 のラインは FIQ へ振り替わる（残りは IRQ）。
//! - `SOFTINT` / `SOFTINTCLEAR` はソフトウェア割り込み。RAWINTR に現れ、
//!   INTENABLE / INTSELECT の対象になる。
//! - `VECTADDR` 読み出しは IACK: 優先度が最も高い（スロット番号が最小の）
//!   有効スロットのベクタアドレスを返し、そのスロットをスタックへ積む。
//!   ペンディングが無いときは `DEFVECTADDR` を返す。書き込みは EOI と
//!   してスタックを一つ積み降ろす。
//! - `PROTECTION` が立っている間は非特権アクセスを `BusFault` で拒否する。
//! - `0xFE0..=0xFEC` は PrimeCell 識別窓（`PrimeCellId`、part 0x190）。
//!
//! 機能モデルの簡略化（宣言的モデル、タイミングを持たない）:
//! - `SWPRIORITYMASK` は値を保持するがソフトウェア優先度マスクの効果は
//!   modeled しない。PL190 のスロット優先度は固定（0 が最高）であり、
//!   マスク無しの優先度解決と同じ結果になる。
//! - `PRIORITYDAISY` はスタンドアロン構成（デイジーチェーン未接続）を
//!   表し、常に 0 を読む。書き込みは無視する。
//! - ベクタ割り込みのエッジトリガ変換と DMA は modeled しない。
//! - IACK のスタックは 16 段。満杯時は最下段を上書きする（QEMU 準拠）。

extern crate alloc;

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, NodeId, SignalId};
use mosaic_ip_arm_primecell_id::PrimeCellId;

/// レジスタオフセット（DDI 0181E 表 3-1）。
mod reg {
    pub const IRQSTATUS: u64 = 0x000;
    pub const FIQSTATUS: u64 = 0x004;
    pub const RAWINTR: u64 = 0x008;
    pub const INTSELECT: u64 = 0x00C;
    pub const INTENABLE: u64 = 0x010;
    pub const INTENCLEAR: u64 = 0x014;
    pub const SOFTINT: u64 = 0x018;
    pub const SOFTINTCLEAR: u64 = 0x01C;
    pub const PROTECTION: u64 = 0x020;
    pub const SWPRIORITYMASK: u64 = 0x024;
    pub const PRIORITYDAISY: u64 = 0x028;
    pub const VECTADDR: u64 = 0x030;
    pub const DEFVECTADDR: u64 = 0x034;
    pub const VECTADDR_SLOT_BASE: u64 = 0x100;
    pub const VECTCNTL_SLOT_BASE: u64 = 0x200;
}

/// 割り込み入力本数。
pub const INPUT_LINES: usize = 32;

/// 優先度スタックの深さ（ベクタスロット数と同じ 16）。
pub const STACK_DEPTH: usize = 16;

/// 構築・操作エラー。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Pl190Error {
    /// ライン番号が 0..32 を超えた。
    LineOutOfRange,
    /// 同じ (source, signal) を複数ラインへ割り当てようとした。
    DuplicateBinding,
    /// すでに別の入力が割り当てられたラインを再利用しようとした。
    LineAlreadyBound,
}

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
pub struct Pl190Params {
    /// 集約済み IRQ 出力（このノードの signal 空間）。
    pub irq: SignalId,
    /// 集約済み FIQ 出力。
    pub fiq: SignalId,
    /// 未マップオフセットの扱い。
    pub unmapped: UnmappedPolicy,
}

impl Pl190Params {
    pub const fn new(irq: SignalId, fiq: SignalId) -> Self {
        Self {
            irq,
            fiq,
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// スナップショット対象のスカラーレジスタ群（設計書 §10 の state 分離）。
/// 入力配線表はホスト側構成であり保存対象外（ADR 0011 のホスト接続原則）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pl190State {
    pub schema: u32,
    /// デバイスが駆動する入力ラインの現在レベル。
    pub hw_lines: u32,
    /// ソフトウェア割り込み（SOFTINT）。
    pub softint: u32,
    /// 割り込みイネーブルマスク。
    pub enable: u32,
    /// 1 のビットのラインは FIQ に振り替わる。
    pub select: u32,
    /// レジスタアクセス保護（bit0）。
    pub protection: bool,
    /// ソフトウェア優先度マスク（値のみ保持、効果は modeled しない）。
    pub swpriority_mask: u16,
    /// スロットごとのベクタアドレス。
    pub vectaddr: [u32; STACK_DEPTH],
    /// スロットごとの制御（bit5 = 有効、bit[4:0] = ライン番号）。
    pub vectcntl: [u8; STACK_DEPTH],
    /// デフォルトベクタアドレス。
    pub defvectaddr: u32,
    /// IACK スタック（スロット番号、-1 = 空）。
    pub stack: [i8; STACK_DEPTH],
    /// 現在の IRQ 出力レベル（エッジ検出用）。
    pub irq_level: bool,
    /// 現在の FIQ 出力レベル。
    pub fiq_level: bool,
}

impl Default for Pl190State {
    fn default() -> Self {
        Self {
            schema: 1,
            hw_lines: 0,
            softint: 0,
            enable: 0,
            select: 0,
            protection: false,
            swpriority_mask: 0xFFFF,
            vectaddr: [0; STACK_DEPTH],
            vectcntl: [0; STACK_DEPTH],
            defvectaddr: 0,
            stack: [-1; STACK_DEPTH],
            irq_level: false,
            fiq_level: false,
        }
    }
}

/// 機能 PL190。割り込み入力は [`Pl190::bind_input`] で注入する。
pub struct Pl190 {
    params: Pl190Params,
    state: Pl190State,
    /// (source node, signal id) → ライン番号 の逆引き表。
    routing: [Option<(NodeId, SignalId)>; INPUT_LINES],
    identity: PrimeCellId,
}

impl Pl190 {
    /// PL190 part number 0x190、revision 0、patch 0。
    pub const PART_NUMBER: u16 = 0x190;

    pub fn new(params: Pl190Params) -> Self {
        Self {
            params,
            state: Pl190State::default(),
            routing: core::array::from_fn(|_| None),
            identity: PrimeCellId::arm(Self::PART_NUMBER, 0, 0)
                .expect("PL190 part number fits the ID fields"),
        }
    }

    /// 割り込み入力を配線する。`source` の出力 `signal` がライン `line`
    /// （0..32）に入る。同じ (source, signal) の二重割当ては構成誤り。
    pub fn bind_input(
        &mut self,
        line: u8,
        source: NodeId,
        signal: SignalId,
    ) -> Result<(), Pl190Error> {
        if usize::from(line) >= INPUT_LINES {
            return Err(Pl190Error::LineOutOfRange);
        }
        let index = usize::from(line);
        if self.routing[index].is_some() {
            return Err(Pl190Error::LineAlreadyBound);
        }
        if self.routing.iter().any(|slot| *slot == Some((source, signal))) {
            return Err(Pl190Error::DuplicateBinding);
        }
        self.routing[index] = Some((source, signal));
        Ok(())
    }

    /// 現在の生ライン（ハード + ソフトウェア）。
    fn lines(&self) -> u32 {
        self.state.hw_lines | self.state.softint
    }

    /// IRQ 出力を主張すべきライン群。
    fn irq_mask(&self) -> u32 {
        self.lines() & self.state.enable & !self.state.select
    }

    /// FIQ 出力を主張すべきライン群。
    fn fiq_mask(&self) -> u32 {
        self.lines() & self.state.enable & self.state.select
    }

    /// 集約出力の再計算と変化エッジの送出（§13.4 rule 1: Ctx 経由のみ）。
    fn update_outputs(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let irq = self.irq_mask() != 0;
        let fiq = self.fiq_mask() != 0;
        let mut first_error = None;
        if irq != self.state.irq_level {
            match set_optional_signal(ctx, self.params.irq, irq) {
                Ok(()) => self.state.irq_level = irq,
                Err(error) => first_error = Some(error),
            }
        }
        // Always attempt both lines; an exhausted queue on IRQ must not block
        // an independent FIQ transition. Each cached level changes only after
        // its own edge was accepted, so a later access can retry either edge.
        if fiq != self.state.fiq_level {
            match set_optional_signal(ctx, self.params.fiq, fiq) {
                Ok(()) => self.state.fiq_level = fiq,
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// IACK: 最高優先度（スロット番号最小）の有効スロットを解決し、
    /// スタックへ積んでベクタアドレスを返す。ペンディング無しは
    /// `DEFVECTADDR` を返し、スタックは更新しない。
    fn iack(&mut self) -> u32 {
        for slot in 0..STACK_DEPTH {
            let control = self.state.vectcntl[slot];
            if control & 0x20 == 0 {
                continue;
            }
            let line = u32::from(control & 0x1F);
            if self.irq_mask() & (1 << line) != 0 {
                // 先頭の空き位置に積む。満杯なら最下段を上書きする。
                let position = self
                    .state
                    .stack
                    .iter()
                    .position(|entry| *entry == -1)
                    .unwrap_or(STACK_DEPTH - 1);
                self.state.stack[position] = slot as i8;
                return self.state.vectaddr[slot];
            }
        }
        self.state.defvectaddr
    }

    /// EOI: スタックの最も新しいエントリを積み降ろす。
    fn eoi(&mut self) {
        if let Some(position) = self.state.stack.iter().rposition(|entry| *entry != -1) {
            self.state.stack[position] = -1;
        }
    }

    /// `PROTECTION` が立っている場合の非特権アクセス拒否。
    fn check_access(&self, attr: &Attr) -> Result<(), BusFault> {
        if self.state.protection && !attr.privileged {
            return Err(BusFault::Device);
        }
        Ok(())
    }

    fn slot_index(offset: u64, base: u64) -> Option<usize> {
        if offset < base || offset >= base + STACK_DEPTH as u64 * 4 {
            return None;
        }
        Some(((offset - base) / 4) as usize)
    }

    fn read_word(&mut self, offset: u64) -> Result<u32, BusFault> {
        let value = match offset {
            reg::IRQSTATUS => self.irq_mask(),
            reg::FIQSTATUS => self.fiq_mask(),
            reg::RAWINTR => self.lines(),
            reg::INTSELECT => self.state.select,
            reg::INTENABLE => self.state.enable,
            reg::INTENCLEAR => 0,
            reg::SOFTINT => self.state.softint,
            reg::SOFTINTCLEAR => 0,
            reg::PROTECTION => u32::from(self.state.protection),
            reg::SWPRIORITYMASK => u32::from(self.state.swpriority_mask),
            reg::PRIORITYDAISY => 0,
            reg::VECTADDR => self.iack(),
            reg::DEFVECTADDR => self.state.defvectaddr,
            _ => {
                if let Some(slot) = Self::slot_index(offset, reg::VECTADDR_SLOT_BASE) {
                    self.state.vectaddr[slot]
                } else if let Some(slot) = Self::slot_index(offset, reg::VECTCNTL_SLOT_BASE) {
                    u32::from(self.state.vectcntl[slot])
                } else if PrimeCellId::covers(offset) {
                    self.identity.read_word(offset).unwrap_or(0)
                } else {
                    return self.unmapped(offset);
                }
            }
        };
        Ok(value)
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u32) -> Result<(), BusFault> {
        match offset {
            reg::IRQSTATUS | reg::FIQSTATUS | reg::RAWINTR | reg::PRIORITYDAISY => {
                // 読み出し専用レジスタへの書き込みは無視する。
            }
            reg::INTSELECT => self.state.select = value,
            reg::INTENABLE => self.state.enable |= value,
            reg::INTENCLEAR => self.state.enable &= !value,
            reg::SOFTINT => self.state.softint |= value,
            reg::SOFTINTCLEAR => self.state.softint &= !value,
            reg::PROTECTION => self.state.protection = value & 1 != 0,
            reg::SWPRIORITYMASK => self.state.swpriority_mask = (value & 0xFFFF) as u16,
            reg::VECTADDR => self.eoi(),
            reg::DEFVECTADDR => self.state.defvectaddr = value,
            _ => {
                if let Some(slot) = Self::slot_index(offset, reg::VECTADDR_SLOT_BASE) {
                    self.state.vectaddr[slot] = value;
                } else if let Some(slot) = Self::slot_index(offset, reg::VECTCNTL_SLOT_BASE) {
                    self.state.vectcntl[slot] = (value & 0x3F) as u8;
                } else if PrimeCellId::covers(offset) {
                    // 識別レジスタは読み出し専用。
                } else {
                    return self.unmapped(offset).map(|_| ());
                }
            }
        }
        self.update_outputs(ctx).map_err(|_| BusFault::Device)?;
        Ok(())
    }

    fn unmapped(&self, _offset: u64) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Pl190 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault> {
        self.check_access(attr)?;
        if matches!(size, AccessSize::DoubleWord) {
            // Reject unsupported widths before a read such as VECTADDR can
            // acknowledge an interrupt or mutate the priority stack.
            return Err(BusFault::InvalidSize);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(offset)?;
        Ok(u64::from(word & mask))
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault> {
        self.check_access(attr)?;
        if matches!(size, AccessSize::DoubleWord) {
            // PL190 レジスタは 32 ビット幅。8 バイト転送は収まらない。
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for Pl190 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        source: NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        // 配線された入力だけを集約する。未配線の源は任意ポートとして無視。
        if let Some(line) = self
            .routing
            .iter()
            .position(|slot| slot == &Some((source, signal)))
        {
            let bit = 1u32 << line;
            if high {
                self.state.hw_lines |= bit;
            } else {
                self.state.hw_lines &= !bit;
            }
            self.update_outputs(ctx)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::vec;

    use super::*;
    use alloc::collections::VecDeque;
    use mosaic_core_types::{EventId, Security, VTime};

    const IRQ: SignalId = SignalId(4);
    const FIQ: SignalId = SignalId(5);
    const UART_NODE: NodeId = NodeId(2);
    const TIMER_NODE: NodeId = NodeId(3);
    const UART_IRQ: SignalId = SignalId(1);
    const TIMER_IRQ: SignalId = SignalId(2);

    /// sp804 テストと同じ形の文脈: タイマキューと信号エッジの記録。
    struct TestCtx {
        now: VTime,
        next_id: u64,
        queue: VecDeque<(VTime, u64, EventId)>,
        edges: Vec<(SignalId, bool)>,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            Self {
                now: VTime::ZERO,
                next_id: 1,
                queue: VecDeque::new(),
                edges: Vec::new(),
            }
        }
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            self.now
        }
        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            let id = EventId(self.next_id);
            self.next_id += 1;
            self.queue.push_back((at, token, id));
            Ok(id)
        }
        fn cancel(&mut self, event: EventId) -> bool {
            if let Some(pos) = self.queue.iter().position(|(_, _, id)| *id == event) {
                self.queue.remove(pos);
                true
            } else {
                false
            }
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.edges.push((signal, high));
            Ok(())
        }
    }

    fn vic() -> Pl190 {
        let mut vic = Pl190::new(Pl190Params::new(IRQ, FIQ));
        vic.bind_input(1, UART_NODE, UART_IRQ).unwrap();
        vic.bind_input(2, TIMER_NODE, TIMER_IRQ).unwrap();
        vic
    }

    fn attr(privileged: bool) -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::Secure,
            privileged,
            kind: mosaic_core_types::AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: mosaic_core_types::Endian::Little,
        }
    }

    fn read(vic: &mut Pl190, ctx: &mut TestCtx, offset: u64) -> u32 {
        vic.read(ctx, offset, AccessSize::Word, &attr(true))
            .unwrap() as u32
    }

    fn write(vic: &mut Pl190, ctx: &mut TestCtx, offset: u64, value: u32) {
        vic.write(ctx, offset, AccessSize::Word, u64::from(value), &attr(true))
            .unwrap();
    }

    fn assert_line(vic: &mut Pl190, ctx: &mut TestCtx, node: NodeId, signal: SignalId, high: bool) {
        vic.on_signal(ctx, node, signal, high).unwrap();
    }

    #[test]
    fn bind_input_validates_lines_and_duplicates() {
        let mut vic = Pl190::new(Pl190Params::new(IRQ, FIQ));
        assert_eq!(
            vic.bind_input(32, UART_NODE, UART_IRQ),
            Err(Pl190Error::LineOutOfRange)
        );
        vic.bind_input(0, UART_NODE, UART_IRQ).unwrap();
        assert_eq!(
            vic.bind_input(1, UART_NODE, UART_IRQ),
            Err(Pl190Error::DuplicateBinding)
        );
        assert_eq!(
            vic.bind_input(0, TIMER_NODE, TIMER_IRQ),
            Err(Pl190Error::LineAlreadyBound)
        );
    }

    #[test]
    fn enabled_lines_drive_irq_and_fiq_outputs_with_edge_tracking() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        // 未イネーブルのラインは RAWINTR にだけ現れる。
        assert_line(&mut vic, &mut ctx, UART_NODE, UART_IRQ, true);
        assert_eq!(read(&mut vic, &mut ctx, reg::RAWINTR), 1 << 1);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 0);
        assert!(ctx.edges.is_empty());

        // イネーブルで IRQ が立ち、FIQ への振替えも反映される。
        write(&mut vic, &mut ctx, reg::INTENABLE, 1 << 1);
        assert_eq!(ctx.edges, vec![(IRQ, true)]);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 1 << 1);
        write(&mut vic, &mut ctx, reg::INTSELECT, 1 << 1);
        assert_eq!(ctx.edges, vec![(IRQ, true), (IRQ, false), (FIQ, true)]);
        assert_eq!(read(&mut vic, &mut ctx, reg::FIQSTATUS), 1 << 1);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 0);

        // ラインが下がると FIQ も下がる。
        assert_line(&mut vic, &mut ctx, UART_NODE, UART_IRQ, false);
        assert_eq!(ctx.edges.last(), Some(&(FIQ, false)));

        // INTENCLEAR でイネーブルを落とす（write-1-to-clear）。
        assert_line(&mut vic, &mut ctx, UART_NODE, UART_IRQ, true);
        write(&mut vic, &mut ctx, reg::INTENCLEAR, 1 << 1);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 0);
    }

    #[test]
    fn software_interrupts_flow_through_the_same_aggregation() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        write(&mut vic, &mut ctx, reg::INTENABLE, 1 << 5);
        write(&mut vic, &mut ctx, reg::SOFTINT, 1 << 5);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 1 << 5);
        assert_eq!(read(&mut vic, &mut ctx, reg::SOFTINT), 1 << 5);
        write(&mut vic, &mut ctx, reg::SOFTINTCLEAR, 1 << 5);
        assert_eq!(read(&mut vic, &mut ctx, reg::IRQSTATUS), 0);
        assert_eq!(read(&mut vic, &mut ctx, reg::SOFTINT), 0);
    }

    #[test]
    fn vectaddr_read_resolves_priority_and_write_pops_the_stack() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        // スロット 0 = ライン 2（TIMER）、スロット 1 = ライン 1（UART）。
        // スロット 0 が最高優先度。
        write(&mut vic, &mut ctx, reg::VECTADDR_SLOT_BASE + 4, 0x18);
        write(&mut vic, &mut ctx, reg::VECTADDR_SLOT_BASE + 8, 0x1C);
        write(&mut vic, &mut ctx, reg::VECTCNTL_SLOT_BASE + 4, 0x20 | 2);
        write(&mut vic, &mut ctx, reg::VECTCNTL_SLOT_BASE + 8, 0x20 | 1);
        write(&mut vic, &mut ctx, reg::INTENABLE, (1 << 1) | (1 << 2));

        // ペンディング無し: DEFVECTADDR。
        write(&mut vic, &mut ctx, reg::DEFVECTADDR, 0x40);
        assert_eq!(read(&mut vic, &mut ctx, reg::VECTADDR), 0x40);

        // 両方ペンディングならスロット 0（TIMER 優先）。IACK は in-service
        // をマスクしない（実機・QEMU 準拠）ので、連続読み出しでも同じ
        // ベクタが積まれ続ける。
        assert_line(&mut vic, &mut ctx, UART_NODE, UART_IRQ, true);
        assert_line(&mut vic, &mut ctx, TIMER_NODE, TIMER_IRQ, true);
        assert_eq!(read(&mut vic, &mut ctx, reg::VECTADDR), 0x18);
        assert_eq!(read(&mut vic, &mut ctx, reg::VECTADDR), 0x18);

        // EOI 書き込みはスタックを一つ降ろす。
        write(&mut vic, &mut ctx, reg::VECTADDR, 0);

        // TIMER ラインが落ちると次優先の UART（スロット 1）が解決される。
        assert_line(&mut vic, &mut ctx, TIMER_NODE, TIMER_IRQ, false);
        assert_eq!(read(&mut vic, &mut ctx, reg::VECTADDR), 0x1C);
    }

    #[test]
    fn protection_register_blocks_unprivileged_access() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        write(&mut vic, &mut ctx, reg::PROTECTION, 1);
        let unprivileged = attr(false);
        assert_eq!(
            vic.read(&mut ctx, reg::IRQSTATUS, AccessSize::Word, &unprivileged),
            Err(BusFault::Device)
        );
        assert_eq!(
            vic.write(&mut ctx, reg::INTENABLE, AccessSize::Word, 1, &unprivileged),
            Err(BusFault::Device)
        );
        // 特権アクセスは通る。
        assert_eq!(read(&mut vic, &mut ctx, reg::PROTECTION), 1);
    }

    #[test]
    fn primecell_identity_window_reports_pl190() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        // PID0 = part[7:0] = 0x90、PID1 = designer/revision ニブル。
        assert_eq!(read(&mut vic, &mut ctx, 0xFE0) & 0xFF, 0x90);
        assert_eq!(read(&mut vic, &mut ctx, 0xFE4) & 0xFF, 0x11);
    }

    #[test]
    fn unmapped_offsets_follow_the_policy() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        assert_eq!(read(&mut vic, &mut ctx, 0x0FC), 0);
        let mut strict = Pl190::new(Pl190Params::new(IRQ, FIQ));
        strict.params.unmapped = UnmappedPolicy::Fault;
        assert_eq!(
            strict.read(&mut ctx, 0x0FC, AccessSize::Word, &attr(true)),
            Err(BusFault::Device)
        );
    }

    #[test]
    fn double_word_transfers_are_rejected() {
        let mut vic = vic();
        let mut ctx = TestCtx::default();
        assert_eq!(
            vic.write(
                &mut ctx,
                reg::INTENABLE,
                AccessSize::DoubleWord,
                1,
                &attr(true)
            ),
            Err(BusFault::InvalidSize)
        );
    }
}
