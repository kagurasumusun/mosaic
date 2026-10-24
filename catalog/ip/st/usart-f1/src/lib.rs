//! STM32F1 USART の機能レジスタモデル (RM0008 §27)。
//!
//! レジスタは SR/DR/BRR/CR1/CR2/CR3/GTPR。送信は DR への書き込みで
//! [`ByteStream`] ポート (任意) へ 1 バイトずつ流れる — PL011 モデルと同じ
//! 即時ドレイン方式 (TX FIFO 無しのポーリング送信に相当し、TXE/TC は
//! 常に 1)。受信は同じポートの `read_bytes` から 1 バイトの RDR と
//! 1 バイトのシフトレジスタへ取り込み、RM0008 §27.6.1 の RXNE / ORE と
//! 「SR を読んでから DR を読む」クリア手順に従う。
//!
//! - **クロックゲーティング**: RCC の APBxENR.USARTxEN 入力
//!   ([`StUsartF1::bind_clock`]) が閉じている間は読み出し 0・書き込み破棄
//!   (実機の APB ブリッジ挙動)。
//! - **送信条件**: CR1 の UE と TE がともに立っていること。DR への書き込みは
//!   TDR であり、読み出しの RDR とは別レジスタである。9 ビットモード M=1 は
//!   送信幅にも受信幅にも影響しない。
//! - **受信条件**: UE と RE がともに立っていること。デバッグ属性の読み出しは
//!   RDR を消費せず、シフトレジスタも進めない。
//! - **割り込み出力**: RXNE/ORE と RXNEIE、TC と TCIE、TXE と TXEIE の OR を
//!   1 本の `combined` IRQ 出力へ結合する。
//! - **BRR**: 値の保持のみ。ボーレートの検証 (PCLK との突き合わせ) は
//!   構成ルート側の責任。
//!
//! 未実装の境界: LIN/スマートカード/SMBus/CTS ハンドシェイク、DMA、
//! IDLE/PE/FE/NE、9 ビットフレーム。予約領域は raz-wi。

#![forbid(unsafe_code)]

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_contract_stream::ByteStream;
use mosaic_core_types::{
    AccessKind, AccessSize, Attr, BusFault, Fnv64, NodeId, SignalId, Snapshot, SnapshotError,
};

/// SR のリセット値: TXE | TC。
const SR_RESET: u32 = 0x00C0;

/// CR1 ビット。
mod cr1 {
    pub const RE: u32 = 1 << 2;
    pub const TE: u32 = 1 << 3;
    pub const RXNEIE: u32 = 1 << 5;
    pub const TCIE: u32 = 1 << 6;
    pub const TXEIE: u32 = 1 << 7;
    pub const UE: u32 = 1 << 13;
    pub const MASK: u32 = 0x3FFF;
}

/// SR ビット。
mod sr {
    pub const ORE: u32 = 1 << 3;
    pub const RXNE: u32 = 1 << 5;
    pub const TC: u32 = 1 << 6;
    pub const TXE: u32 = 1 << 7;
}

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct StUsartF1Params {
    /// 結合割り込み出力のシグナル ID。
    pub irq: SignalId,
}

impl StUsartF1Params {
    pub fn new(irq: SignalId) -> Self {
        Self { irq }
    }
}

/// STM32F1 USART の機能モデル。
pub struct StUsartF1 {
    params: StUsartF1Params,
    /// 送信先バイトストリーム (オプション)。
    stream: Option<Box<dyn ByteStream>>,
    clock_input: Option<(NodeId, SignalId)>,
    /// レジスタの保持値。DR の書き込みは TDR、読み出しは RDR（RM0008）。
    sr: u32,
    tdr: u32,
    rdr: u8,
    /// 受信シフトレジスタ。RDR が埋まった次の 1 バイトを保持する。
    shift: u8,
    shift_valid: bool,
    /// SR を読んだあと DR を読むと ORE が落ちる、という手順の途中。
    sr_read: bool,
    brr: u32,
    cr1: u32,
    cr2: u32,
    cr3: u32,
    gtpr: u32,
    clock_on: bool,
    /// 割り込み出力の現在レベル。
    irq_level: bool,
}

impl StUsartF1 {
    pub fn new(params: StUsartF1Params, stream: Option<Box<dyn ByteStream>>) -> Self {
        Self {
            params,
            stream,
            clock_input: None,
            sr: SR_RESET,
            tdr: 0,
            rdr: 0,
            shift: 0,
            shift_valid: false,
            sr_read: false,
            brr: 0,
            cr1: 0,
            cr2: 0,
            cr3: 0,
            gtpr: 0,
            clock_on: false,
            irq_level: false,
        }
    }

    /// RCC のクロックゲート入力を配線する。
    pub fn bind_clock(&mut self, source: NodeId, signal: SignalId) {
        self.clock_input = Some((source, signal));
    }

    fn receiver_enabled(&self) -> bool {
        self.clock_on && self.cr1 & (cr1::UE | cr1::RE) == (cr1::UE | cr1::RE)
    }

    fn pull_rx(&mut self, ctx: &mut dyn Ctx) -> Option<u8> {
        let port = self.stream.as_mut()?;
        let mut byte = [0u8; 1];
        match port.read_bytes(ctx, &mut byte) {
            Ok(1) => Some(byte[0]),
            _ => None,
        }
    }

    /// Move a completed shift byte into RDR, then accept at most one more
    /// wire byte. A third byte while both registers are full is the overrun
    /// RM0008 describes: the new character is lost and ORE is set.
    fn ingest(&mut self, ctx: &mut dyn Ctx) {
        if !self.receiver_enabled() {
            return;
        }
        if !self.shift_valid {
            if let Some(byte) = self.pull_rx(ctx) {
                self.shift = byte;
                self.shift_valid = true;
            }
        }
        if self.sr & sr::RXNE == 0 && self.shift_valid {
            self.rdr = self.shift;
            self.shift_valid = false;
            self.sr |= sr::RXNE;
            if let Some(byte) = self.pull_rx(ctx) {
                self.shift = byte;
                self.shift_valid = true;
            }
        }
        if self.sr & sr::RXNE != 0 && self.shift_valid && self.pull_rx(ctx).is_some() {
            self.sr |= sr::ORE;
        }
    }

    fn take_rdr(&mut self) -> u8 {
        let byte = self.rdr;
        self.sr &= !sr::RXNE;
        if self.sr_read {
            self.sr &= !sr::ORE;
            self.sr_read = false;
        }
        if self.shift_valid {
            self.rdr = self.shift;
            self.shift_valid = false;
            self.sr |= sr::RXNE;
        }
        byte
    }

    /// 結合割り込み出力の再評価。ORE は RXNEIE で通知する（RM0008 §27.4）。
    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = self.sr & (sr::RXNE | sr::ORE) != 0 && self.cr1 & cr1::RXNEIE != 0
            || self.sr & sr::TC != 0 && self.cr1 & cr1::TCIE != 0
            || self.sr & sr::TXE != 0 && self.cr1 & cr1::TXEIE != 0;
        if level != self.irq_level {
            set_optional_signal(ctx, self.params.irq, level)?;
            self.irq_level = level;
        }
        Ok(())
    }
}

impl MmioTarget for StUsartF1 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            return Err(BusFault::InvalidSize);
        }
        if !self.clock_on {
            return Ok(0);
        }
        let debug = attr.kind == AccessKind::Debug;
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = match offset {
            0x00 => {
                if !debug {
                    self.ingest(ctx);
                    self.sr_read = true;
                    self.update_irq(ctx).map_err(|_| BusFault::Device)?;
                }
                self.sr
            }
            0x04 => {
                if debug {
                    u32::from(self.rdr)
                } else {
                    self.ingest(ctx);
                    let byte = self.take_rdr();
                    self.ingest(ctx);
                    self.update_irq(ctx).map_err(|_| BusFault::Device)?;
                    u32::from(byte)
                }
            }
            0x08 => self.brr,
            0x0C => self.cr1,
            0x10 => self.cr2 & 0x7F7F,
            0x14 => self.cr3 & 0x05FF,
            0x18 => self.gtpr & 0x00FF,
            _ => 0,
        };
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
            return Err(BusFault::InvalidSize);
        }
        if value > u64::from(u32::MAX) {
            return Err(BusFault::InvalidSize);
        }
        if !self.clock_on {
            return Ok(());
        }
        let value = value as u32;
        match offset {
            0x00 => {
                // SR への書き込み: 0 を書いた TC / RXNE だけが落ちる (RM0008 §27.6.1)。
                if value & sr::TC == 0 {
                    self.sr &= !sr::TC;
                }
                if value & sr::RXNE == 0 {
                    self.sr &= !sr::RXNE;
                    if self.shift_valid {
                        self.rdr = self.shift;
                        self.shift_valid = false;
                        self.sr |= sr::RXNE;
                    }
                }
                self.ingest(ctx);
                self.update_irq(ctx).map_err(|_| BusFault::Device)?;
            }
            0x04 => {
                // DR 書き込みは TDR。RDR / RXNE には触れない。
                self.tdr = value & 0xFFFF;
                if self.cr1 & (cr1::UE | cr1::TE) == (cr1::UE | cr1::TE) {
                    if let Some(port) = self.stream.as_mut() {
                        let byte = (value & 0xFF) as u8;
                        match port.write_bytes(ctx, core::slice::from_ref(&byte)) {
                            Ok(1) => {}
                            // A one-byte transfer must make exactly one byte
                            // of progress. Surface back-pressure instead of
                            // silently dropping the guest's transmitted byte.
                            _ => return Err(BusFault::Device),
                        }
                    }
                }
            }
            0x08 => self.brr = value & 0xFFFF,
            0x0C => {
                self.cr1 = value & cr1::MASK;
                self.ingest(ctx);
                self.update_irq(ctx).map_err(|_| BusFault::Device)?;
            }
            0x10 => self.cr2 = value & 0x7F7F,
            0x14 => self.cr3 = value & 0x05FF,
            0x18 => self.gtpr = value & 0x00FF,
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for StUsartF1 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        source: NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        if self.clock_input == Some((source, signal)) {
            self.clock_on = high;
        }
        Ok(())
    }
}

/// USART checkpoint. The byte stream and clock-gate binding are host ports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StUsartF1Checkpoint {
    pub schema: u32,
    pub sr: u32,
    pub tdr: u32,
    pub rdr: u8,
    pub shift: u8,
    pub shift_valid: bool,
    pub sr_read: bool,
    pub brr: u32,
    pub cr1: u32,
    pub cr2: u32,
    pub cr3: u32,
    pub gtpr: u32,
    pub clock_on: bool,
    pub irq_level: bool,
}

impl Snapshot for StUsartF1 {
    type State = StUsartF1Checkpoint;
    const SCHEMA_VERSION: u32 = 2;

    fn save(&self) -> Self::State {
        StUsartF1Checkpoint {
            schema: Self::SCHEMA_VERSION,
            sr: self.sr,
            tdr: self.tdr,
            rdr: self.rdr,
            shift: self.shift,
            shift_valid: self.shift_valid,
            sr_read: self.sr_read,
            brr: self.brr,
            cr1: self.cr1,
            cr2: self.cr2,
            cr3: self.cr3,
            gtpr: self.gtpr,
            clock_on: self.clock_on,
            irq_level: self.irq_level,
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.sr = state.sr;
        self.tdr = state.tdr;
        self.rdr = state.rdr;
        self.shift = state.shift;
        self.shift_valid = state.shift_valid;
        self.sr_read = state.sr_read;
        self.brr = state.brr;
        self.cr1 = state.cr1;
        self.cr2 = state.cr2;
        self.cr3 = state.cr3;
        self.gtpr = state.gtpr;
        self.clock_on = state.clock_on;
        self.irq_level = state.irq_level;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        for word in [
            state.sr, state.tdr, state.brr, state.cr1, state.cr2, state.cr3, state.gtpr,
        ] {
            hash.u32(word);
        }
        hash.byte(state.rdr);
        hash.byte(state.shift);
        hash.bool(state.shift_valid);
        hash.bool(state.sr_read);
        hash.bool(state.clock_on);
        hash.bool(state.irq_level);
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
    use std::collections::VecDeque;

    use mosaic_contract_stream::StreamError;
    use mosaic_core_types::{AccessKind, EventId, VTime};

    const IRQ: SignalId = SignalId(4);

    #[derive(Default)]
    struct VecStream {
        tx: Vec<u8>,
        rx: VecDeque<u8>,
    }

    impl ByteStream for VecStream {
        fn write_bytes(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
            self.tx.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn read_bytes(&mut self, _ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, StreamError> {
            let Some(slot) = buf.first_mut() else {
                return Ok(0);
            };
            let Some(byte) = self.rx.pop_front() else {
                return Ok(0);
            };
            *slot = byte;
            Ok(1)
        }
    }

    /// テスト用ピア: 記録ストリームを契約型の裏で共有する。
    struct TestStream {
        inner: Arc<Mutex<VecStream>>,
    }

    impl ByteStream for TestStream {
        fn write_bytes(&mut self, ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
            self.inner.lock().unwrap().write_bytes(ctx, bytes)
        }
        fn read_bytes(&mut self, ctx: &mut dyn Ctx, buf: &mut [u8]) -> Result<usize, StreamError> {
            self.inner.lock().unwrap().read_bytes(ctx, buf)
        }
    }

    struct RecordingCtx {
        edges: Vec<(SignalId, bool)>,
    }

    impl Ctx for RecordingCtx {
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
            self.edges.push((signal, high));
            Ok(())
        }
    }

    fn word_attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: mosaic_core_types::Security::NonSecure,
            privileged: false,
            kind: mosaic_core_types::AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: mosaic_core_types::Endian::Little,
        }
    }

    fn read(uart: &mut StUsartF1, ctx: &mut RecordingCtx, offset: u64) -> u32 {
        uart.read(ctx, offset, AccessSize::Word, &word_attr())
            .unwrap() as u32
    }

    fn write(uart: &mut StUsartF1, ctx: &mut RecordingCtx, offset: u64, value: u32) {
        uart.write(
            ctx,
            offset,
            AccessSize::Word,
            u64::from(value),
            &word_attr(),
        )
        .unwrap();
    }

    fn uart() -> (
        StUsartF1,
        RecordingCtx,
        Arc<Mutex<VecStream>>,
        NodeId,
        SignalId,
    ) {
        let stream = Arc::new(Mutex::new(VecStream::default()));
        let mut uart = StUsartF1::new(
            StUsartF1Params::new(IRQ),
            Some(Box::new(TestStream {
                inner: Arc::clone(&stream),
            })),
        );
        let (source, signal) = (NodeId(3), SignalId(0x3E));
        uart.bind_clock(source, signal);
        let mut ctx = RecordingCtx { edges: Vec::new() };
        uart.on_signal(&mut ctx, source, signal, true).unwrap();
        (uart, ctx, stream, source, signal)
    }

    #[test]
    fn clock_gate_hides_the_registers() {
        let (mut uart, mut ctx, _stream, source, signal) = uart();
        assert_eq!(read(&mut uart, &mut ctx, 0x00), SR_RESET);
        uart.on_signal(&mut ctx, source, signal, false).unwrap();
        assert_eq!(read(&mut uart, &mut ctx, 0x00), 0);
        // ゲートが閉じている間の書き込みは破棄される。
        write(&mut uart, &mut ctx, 0x0C, cr1::UE | cr1::TE);
        uart.on_signal(&mut ctx, source, signal, true).unwrap();
        assert_eq!(read(&mut uart, &mut ctx, 0x0C), 0);
    }

    #[test]
    fn transmit_requires_ue_and_te() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        // UE/TE なし: DR への書き込みは保持されるが流れない。
        write(&mut uart, &mut ctx, 0x04, b'x' as u32);
        // DR 読み出しは RDR であり、送ったバイトは戻らない。
        assert_eq!(read(&mut uart, &mut ctx, 0x04), 0);
        assert!(stream.lock().unwrap().tx.is_empty());
        // TE のみ: まだ流れない。
        write(&mut uart, &mut ctx, 0x0C, cr1::TE);
        write(&mut uart, &mut ctx, 0x04, b'y' as u32);
        assert!(stream.lock().unwrap().tx.is_empty());
        // UE|TE: 流れる。
        write(&mut uart, &mut ctx, 0x0C, cr1::UE | cr1::TE);
        write(&mut uart, &mut ctx, 0x04, b'h' as u32);
        write(&mut uart, &mut ctx, 0x04, b'i' as u32);
        assert_eq!(stream.lock().unwrap().tx, b"hi".to_vec());
        // SR は TXE|TC 常時立ちは即時ドレイン相当。
        assert_eq!(read(&mut uart, &mut ctx, 0x00), sr::TXE | sr::TC);
    }

    #[test]
    fn txeie_drives_the_combined_irq_output() {
        let (mut uart, mut ctx, _stream, _source, _signal) = uart();
        // TXE は常時 1 のため、TXEIE を立てると出力が立つ。
        write(&mut uart, &mut ctx, 0x0C, cr1::UE | cr1::TE | cr1::TXEIE);
        assert_eq!(ctx.edges, vec![(IRQ, true)]);
        // SR への 0 書き込みは TC をクリアするが TXE は残るため高いまま。
        write(&mut uart, &mut ctx, 0x00, 0);
        assert_eq!(ctx.edges.len(), 1);
        // TXEIE を外すと出力が下がる。
        write(&mut uart, &mut ctx, 0x0C, cr1::UE | cr1::TE);
        assert_eq!(ctx.edges.last(), Some(&(IRQ, false)));
    }

    #[test]
    fn brr_cr2_cr3_gtpr_storage() {
        let (mut uart, mut ctx, _stream, _source, _signal) = uart();
        write(&mut uart, &mut ctx, 0x08, 0x22B9); // 72MHz/9600 相当
        assert_eq!(read(&mut uart, &mut ctx, 0x08), 0x22B9);
        write(&mut uart, &mut ctx, 0x10, 0x2000); // STOP=10
        assert_eq!(read(&mut uart, &mut ctx, 0x10), 0x2000);
        write(&mut uart, &mut ctx, 0x14, 0x01FF);
        assert_eq!(read(&mut uart, &mut ctx, 0x14), 0x01FF);
        write(&mut uart, &mut ctx, 0x18, 0x0234);
        assert_eq!(read(&mut uart, &mut ctx, 0x18), 0x0034);
    }

    fn enable_receiver(uart: &mut StUsartF1, ctx: &mut RecordingCtx) {
        write(uart, ctx, 0x0C, cr1::UE | cr1::RE);
    }

    #[test]
    fn receive_moves_one_byte_into_rdr_and_dr_read_clears_rxne() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        stream.lock().unwrap().rx.push_back(b'M');
        enable_receiver(&mut uart, &mut ctx);
        assert_eq!(read(&mut uart, &mut ctx, 0x00) & sr::RXNE, sr::RXNE);
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'M'));
        assert_eq!(read(&mut uart, &mut ctx, 0x00) & sr::RXNE, 0);
    }

    #[test]
    fn a_third_waiting_byte_sets_ore_until_sr_then_dr() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        stream.lock().unwrap().rx.extend([b'a', b'b', b'c']);
        enable_receiver(&mut uart, &mut ctx);
        let status = read(&mut uart, &mut ctx, 0x00);
        assert_eq!(status & (sr::RXNE | sr::ORE), sr::RXNE | sr::ORE);
        // SR を読んだあとなので、DR 読み出しは先頭バイトを返し ORE を落とす。
        // シフトレジスタの次バイトが RDR へ移る。
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'a'));
        assert_eq!(read(&mut uart, &mut ctx, 0x00) & sr::ORE, 0);
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'b'));
        // RXNE は落ちるが、RDR は最後に受信したバイトを保持する（RM0008）。
        assert_eq!(read(&mut uart, &mut ctx, 0x00) & sr::RXNE, 0);
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'b'));
    }

    #[test]
    fn rxneie_follows_the_receive_flag() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        stream.lock().unwrap().rx.push_back(b'!');
        write(&mut uart, &mut ctx, 0x0C, cr1::UE | cr1::RE | cr1::RXNEIE);
        assert_eq!(ctx.edges.last(), Some(&(IRQ, true)));
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'!'));
        assert_eq!(ctx.edges.last(), Some(&(IRQ, false)));
    }

    #[test]
    fn debug_read_does_not_consume_rdr() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        stream.lock().unwrap().rx.push_back(b'Q');
        enable_receiver(&mut uart, &mut ctx);
        let _ = read(&mut uart, &mut ctx, 0x00);
        let mut debug = word_attr();
        debug.kind = AccessKind::Debug;
        assert_eq!(
            uart.read(&mut ctx, 0x04, AccessSize::Word, &debug).unwrap(),
            u64::from(b'Q')
        );
        assert_eq!(read(&mut uart, &mut ctx, 0x00) & sr::RXNE, sr::RXNE);
        assert_eq!(read(&mut uart, &mut ctx, 0x04), u32::from(b'Q'));
    }

    #[test]
    fn receive_state_roundtrips_without_the_host_stream() {
        let (mut uart, mut ctx, stream, _source, _signal) = uart();
        stream.lock().unwrap().rx.extend([b'z', b'y']);
        enable_receiver(&mut uart, &mut ctx);
        let _ = read(&mut uart, &mut ctx, 0x00);
        let saved = uart.save();
        let mut restored = StUsartF1::new(StUsartF1Params::new(IRQ), None);
        restored.load(StUsartF1::SCHEMA_VERSION, &saved).unwrap();
        assert_eq!(
            StUsartF1::fingerprint(StUsartF1::SCHEMA_VERSION, &saved),
            StUsartF1::fingerprint(StUsartF1::SCHEMA_VERSION, &restored.save())
        );
        assert_eq!(read(&mut restored, &mut ctx, 0x04), u32::from(b'z'));
    }
}
