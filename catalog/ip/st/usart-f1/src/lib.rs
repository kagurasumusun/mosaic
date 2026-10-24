//! STM32F1 USART の機能レジスタモデル (RM0008 §27)。
//!
//! レジスタは SR/DR/BRR/CR1/CR2/CR3/GTPR。送信は DR への書き込みで
//! [`ByteStream`] ポート (任意) へ 1 バイトずつ流れる — PL011 モデルと同じ
//! 即時ドレイン方式 (TX FIFO 無しのポーリング送信に相当し、TXE/TC は
//! 常に 1)。
//!
//! - **クロックゲーティング**: RCC の APBxENR.USARTxEN 入力
//!   ([`StUsartF1::bind_clock`]) が閉じている間は読み出し 0・書き込み破棄
//!   (実機の APB ブリッジ挙動)。
//! - **送信条件**: CR1 の UE と TE がともに立っていること。DR への書き込みは
//!   低 8 ビットのみ有効 (9 ビットモード M=1 は本モデルでは送信幅に影響しない)。
//! - **割り込み出力**: TXE/TC/RXNE フラグと対応するイネーブルビット (TXEIE/
//!   TCIE/RXNEIE) の AND を結合して 1 本の `combined` IRQ 出力へ (PL011 と
//!   同じ結合方式)。本モデルでは RX が無いため RXNE は常に 0。
//! - **BRR**: 値の保持のみ。ボーレートの検証 (PCLK との突き合わせ) は
//!   構成ルート側の責任。
//!
//! 未実装の境界: LIN/スマートカード/SMBus/CTS ハンドシェイク、DMA、
//! 受信 (IDLE/RXNE)、エラーフラグの立上り (PE/FE/NE/ORE は 0)。
//! 予約領域は raz-wi。

#![forbid(unsafe_code)]

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_contract_stream::ByteStream;
use mosaic_core_types::{
    AccessSize, Attr, BusFault, Fnv64, NodeId, SignalId, Snapshot, SnapshotError,
};

/// SR のリセット値: TXE | TC。
const SR_RESET: u32 = 0x00C0;

/// CR1 ビット。
mod cr1 {
    pub const TE: u32 = 1 << 3;
    pub const RXNEIE: u32 = 1 << 5;
    pub const TCIE: u32 = 1 << 6;
    pub const TXEIE: u32 = 1 << 7;
    pub const UE: u32 = 1 << 13;
    pub const MASK: u32 = 0x3FFF;
}

/// SR ビット。
mod sr {
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
    /// レジスタの保持値。
    sr: u32,
    dr: u32,
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
            dr: 0,
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

    /// 結合割り込み出力の再評価。
    fn update_irq(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let level = self.sr & sr::RXNE != 0 && self.cr1 & cr1::RXNEIE != 0
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
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            return Err(BusFault::InvalidSize);
        }
        if !self.clock_on {
            return Ok(0);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = match offset {
            0x00 => self.sr,
            0x04 => self.dr,
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
                // SR への書き込み: TC に 0 を書くとクリア (RM0008 §27.6.1)。
                if value & sr::TC == 0 {
                    self.sr &= !sr::TC;
                }
                self.update_irq(ctx).map_err(|_| BusFault::Device)?;
            }
            0x04 => {
                // DR: UE|TE が立っていれば送信 (下 8 ビット)。
                self.dr = value & 0xFFFF;
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
                // 読み出しで RXNE を落とす規則は受信未実装のため省略。
            }
            0x08 => self.brr = value & 0xFFFF,
            0x0C => {
                self.cr1 = value & cr1::MASK;
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
    pub dr: u32,
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
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        StUsartF1Checkpoint {
            schema: Self::SCHEMA_VERSION,
            sr: self.sr,
            dr: self.dr,
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
        self.dr = state.dr;
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
            state.sr, state.dr, state.brr, state.cr1, state.cr2, state.cr3, state.gtpr,
        ] {
            hash.u32(word);
        }
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
    use mosaic_contract_stream::StreamError;
    use mosaic_core_types::{EventId, VTime};

    const IRQ: SignalId = SignalId(4);

    #[derive(Default)]
    struct VecStream {
        tx: Vec<u8>,
    }

    impl ByteStream for VecStream {
        fn write_bytes(&mut self, _ctx: &mut dyn Ctx, bytes: &[u8]) -> Result<usize, StreamError> {
            self.tx.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn read_bytes(
            &mut self,
            _ctx: &mut dyn Ctx,
            _buf: &mut [u8],
        ) -> Result<usize, StreamError> {
            Ok(0) // 受信は未実装。
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
        assert_eq!(read(&mut uart, &mut ctx, 0x04), b'x' as u32);
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
}
