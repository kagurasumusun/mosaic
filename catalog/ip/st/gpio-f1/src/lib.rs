//! STM32F1 GPIO ポートの機能レジスタモデル (RM0008 §9)。
//!
//! 1 ノード = 1 ポート (GPIOA..G)。レジスタは CRL/CRH/IDR/ODR/BSRR/BRR/LCKR。
//!
//! - **クロックゲーティング**: STM32F1 では RCC の APB2ENR.IOPxEN が立つまで
//!   ポートのレジスタは動作しない。モデルは [`GpioF1::bind_clock`] で配線した
//!   RCC ゲート入力 (wired input, 構成設計書 §13.3) を `clock_on` として保持し、
//!   ゲートが閉じている間は**読み出し 0・書き込み破棄** (実機の APB ブリッジ
//!   挙動: クロック無しのペリフェラル読み出しは 0)。
//! - **ピン信号出力**: 汎用出力 (CNF=00, MODE≠00) に設定されたピンの ODR ビット
//!   が変化したとき、そのピン番号の SignalId でレベルを発行する
//!   (16 本の `pins` ポート)。オルタネート出力 (CNF=10) はペリフェラルが駆動
//!   するため、このモデルからは発行しない。入力ピンの IDR は外部ネット未接続
//!   の場合 0 (GPIO ネット相互接続は後続スライス)。
//! - **BSRR**: セット優先 (RM0008: 同一ビットへの BS/BR 同時書き込みは BS が
//!   優先)。BRR はリセット専用。
//! - **LCKR**: LCKK (ビット 16) を立てた書き込みで設定をロックする
//!   (実機の読み出し 2 回を伴うキー列は省略: 最初の LCKK=1 書き込みで確定)。
//!   ロック後の CRL/CRH/LCKR への書き込みは破棄。
//!
//! 予約領域 (0x1C..0x400) は raz-wi。

#![forbid(unsafe_code)]

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{
    AccessSize, Attr, BusFault, Fnv64, NodeId, SignalId, Snapshot, SnapshotError,
};

/// CRL/CRH のリセット値: 全ピン「浮遊入力」(CNF=01, MODE=00)。
const CR_RESET: u32 = 0x4444_4444;

/// ピン n の出力 SignalId。
pub fn pin_signal(pin: u32) -> SignalId {
    SignalId(pin)
}

/// モデル構成パラメータ (将来のポート差分用。現在は差なし)。
#[derive(Clone, Copy, Debug, Default)]
pub struct GpioF1Params;

/// STM32F1 GPIO ポートの機能モデル。
pub struct GpioF1 {
    state: GpioState,
    /// ODR/configuration transitions whose notification could not yet be queued.
    /// These bits are retried on subsequent MMIO writes.
    pending_pins: u16,
    /// bind_clock で配線された RCC ゲート入力。
    clock_input: Option<(NodeId, SignalId)>,
}

#[derive(Clone, Copy, Debug)]
struct GpioState {
    clock_on: bool,
    crl: u32,
    crh: u32,
    odr: u32,
    /// LCKR の保持値 (LCKK を含む)。
    lckr: u32,
}

impl Default for GpioState {
    fn default() -> Self {
        Self {
            clock_on: false,
            crl: CR_RESET,
            crh: CR_RESET,
            odr: 0,
            lckr: 0,
        }
    }
}

impl GpioF1 {
    pub fn new(_params: GpioF1Params) -> Self {
        Self {
            state: GpioState::default(),
            pending_pins: 0,
            clock_input: None,
        }
    }

    /// RCC のクロックゲート入力を配線する: `source` ノードの `signal` が
    /// このポートの APB クロックイネーブル。
    pub fn bind_clock(&mut self, source: NodeId, signal: SignalId) {
        self.clock_input = Some((source, signal));
    }

    /// 設定がロックされているか (LCKK)。
    fn locked(&self) -> bool {
        self.state.lckr & (1 << 16) != 0
    }

    /// ピン `pin` の CNF/MODE ニブル (CRL: 0-7, CRH: 8-15)。
    fn pin_config(&self, pin: u32) -> u32 {
        let (cr, shift) = if pin < 8 {
            (self.state.crl, pin * 4)
        } else {
            (self.state.crh, (pin - 8) * 4)
        };
        (cr >> shift) & 0xF
    }

    /// ピンが汎用出力 (CNF=00, MODE≠00) か。
    fn is_gp_output(&self, pin: u32) -> bool {
        let config = self.pin_config(pin);
        config & 0b1100 == 0 && config & 0b0011 != 0
    }

    /// ODR/configuration changesを汎用出力ピンの信号へ配送する。
    ///
    /// A full runtime queue leaves the corresponding bit pending. A later
    /// access retries it even when the ODR value itself is unchanged, avoiding
    /// a lost output level after a rejected `set_signal`.
    fn emit_pin_changes(&mut self, ctx: &mut dyn Ctx, old: u32) -> Result<(), ContextError> {
        self.pending_pins |= (old ^ self.state.odr) as u16;
        let mut pending = self.pending_pins;
        let mut first_error = None;
        while pending != 0 {
            let pin = pending.trailing_zeros();
            let bit = 1u16 << pin;
            pending &= !bit;
            if !self.is_gp_output(pin) {
                // Keep the bit pending: a later CRL/CRH write may turn this pin
                // into a GPIO output, at which point its current level is sent.
                continue;
            }
            let level = self.state.odr & u32::from(bit) != 0;
            match set_optional_signal(ctx, pin_signal(pin), level) {
                Ok(()) => self.pending_pins &= !bit,
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

    /// IDR の読み出し値。出力設定ピンは ODR を、それ以外は 0 を返す。
    fn idr_read(&self) -> u32 {
        let mut value = 0;
        for pin in 0..16u32 {
            if self.is_gp_output(pin) {
                value |= self.state.odr & (1 << pin);
            }
        }
        value
    }
}

impl MmioTarget for GpioF1 {
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
        if !self.state.clock_on {
            // クロック無しのポート読み出しは 0。
            return Ok(0);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = match offset {
            0x00 => self.state.crl,
            0x04 => self.state.crh,
            0x08 => self.idr_read(),
            0x0C => self.state.odr,
            0x10 => 0, // BSRR は書き込み専用。
            0x14 => 0, // BRR は書き込み専用。
            0x18 => self.state.lckr,
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
        if !self.state.clock_on {
            // クロック無しのポートへの書き込みは破棄。
            return Ok(());
        }
        let value = value as u32;
        let old_odr = self.state.odr;
        let mut config_changed = false;
        match offset {
            0x00 => {
                if !self.locked() && self.state.crl != value {
                    self.state.crl = value;
                    config_changed = true;
                }
            }
            0x04 => {
                if !self.locked() && self.state.crh != value {
                    self.state.crh = value;
                    config_changed = true;
                }
            }
            0x0C => self.state.odr = value & 0xFFFF,
            0x10 => {
                // BSRR: BSy セット + BRy リセット。セット優先。
                let sets = value & 0xFFFF;
                let resets = (value >> 16) & 0xFFFF;
                self.state.odr |= sets;
                self.state.odr &= !(resets & !sets);
            }
            0x14 => self.state.odr &= !(value & 0xFFFF),
            0x18 => {
                if !self.locked() {
                    // LCKK を含めて保持。LCKK=1 の書き込みでロック確定。
                    self.state.lckr = value & 0x0001_FFFF;
                }
            }
            _ => {}
        }
        if config_changed {
            // A pin that just became a GPIO output must emit its current ODR
            // level even if the ODR register itself did not change.
            self.pending_pins = u16::MAX;
        }
        self.emit_pin_changes(ctx, old_odr)
            .map_err(|_| BusFault::Device)?;
        Ok(())
    }
}

impl EventTarget for GpioF1 {
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
            self.state.clock_on = high;
        }
        Ok(())
    }
}

/// GPIO checkpoint. The clock-gate input binding is host wiring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpioF1Checkpoint {
    pub schema: u32,
    pub clock_on: bool,
    pub crl: u32,
    pub crh: u32,
    pub odr: u32,
    pub lckr: u32,
    pub pending_pins: u16,
}

impl Snapshot for GpioF1 {
    type State = GpioF1Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        GpioF1Checkpoint {
            schema: Self::SCHEMA_VERSION,
            clock_on: self.state.clock_on,
            crl: self.state.crl,
            crh: self.state.crh,
            odr: self.state.odr,
            lckr: self.state.lckr,
            pending_pins: self.pending_pins,
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
        self.state.clock_on = state.clock_on;
        self.state.crl = state.crl;
        self.state.crh = state.crh;
        self.state.odr = state.odr;
        self.state.lckr = state.lckr;
        self.pending_pins = state.pending_pins;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.bool(state.clock_on);
        hash.u32(state.crl);
        hash.u32(state.crh);
        hash.u32(state.odr);
        hash.u32(state.lckr);
        hash.u16(state.pending_pins);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::vec;

    use super::*;

    struct TestCtx {
        edges: Vec<(SignalId, bool)>,
        fail_next_signal: bool,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                edges: Vec::new(),
                fail_next_signal: false,
            }
        }

        fn failing_once() -> Self {
            Self {
                edges: Vec::new(),
                fail_next_signal: true,
            }
        }
    }

    impl Ctx for TestCtx {
        fn now(&self) -> mosaic_core_types::VTime {
            mosaic_core_types::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_core_types::VTime,
            _token: u64,
        ) -> Result<mosaic_core_types::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_core_types::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            if self.fail_next_signal {
                self.fail_next_signal = false;
                return Err(ContextError::ResourceExhausted);
            }
            self.edges.push((signal, high));
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: mosaic_core_types::Security::Secure,
            privileged: true,
            kind: mosaic_core_types::AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: mosaic_core_types::Endian::Little,
        }
    }

    fn read(gpio: &mut GpioF1, ctx: &mut TestCtx, offset: u64) -> u32 {
        gpio.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn write(gpio: &mut GpioF1, ctx: &mut TestCtx, offset: u64, value: u32) {
        gpio.write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    fn clocked() -> (GpioF1, TestCtx, NodeId, SignalId) {
        let mut gpio = GpioF1::new(GpioF1Params);
        let source = NodeId(3);
        let signal = SignalId(0x32);
        gpio.bind_clock(source, signal);
        let mut ctx = TestCtx::new();
        gpio.on_signal(&mut ctx, source, signal, true).unwrap();
        (gpio, ctx, source, signal)
    }

    #[test]
    fn clock_gate_hides_and_reveals_the_port() {
        let (mut gpio, mut ctx, source, signal) = clocked();
        // クロックあり: リセット値が読める。
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), CR_RESET);
        write(&mut gpio, &mut ctx, 0x04, 0x4444_44B0);
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), 0x4444_44B0);
        // ゲートを閉じると読み出し 0・書き込み破棄。
        gpio.on_signal(&mut ctx, source, signal, false).unwrap();
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), 0);
        write(&mut gpio, &mut ctx, 0x04, 0);
        // 再び開くと保持値が戻る。
        gpio.on_signal(&mut ctx, source, signal, true).unwrap();
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), 0x4444_44B0);
        // 未配線の入力では変化しない。
        gpio.on_signal(&mut ctx, NodeId(9), SignalId(0x32), false)
            .unwrap();
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), 0x4444_44B0);
    }

    #[test]
    fn bsrr_and_brr_drive_output_pins_and_emit_signals() {
        let (mut gpio, mut ctx, _source, _signal) = clocked();
        // PA5 を汎用出力 50MHz (CNF=00, MODE=11 → ニブル 0x3) に設定。
        let crl = (CR_RESET & !(0xF << 20)) | (0x3 << 20);
        write(&mut gpio, &mut ctx, 0x00, crl);
        // BSRR でセット。
        write(&mut gpio, &mut ctx, 0x10, 1 << 5);
        assert_eq!(read(&mut gpio, &mut ctx, 0x0C), 1 << 5);
        assert_eq!(read(&mut gpio, &mut ctx, 0x08), 1 << 5); // IDR は ODR を反映。
        assert_eq!(
            ctx.edges,
            vec![(pin_signal(5), false), (pin_signal(5), true)]
        );
        // BRR でリセット。
        write(&mut gpio, &mut ctx, 0x14, 1 << 5);
        assert_eq!(read(&mut gpio, &mut ctx, 0x0C), 0);
        assert_eq!(ctx.edges.last(), Some(&(pin_signal(5), false)));
        // BS 優先: 同じビットに BS と BR を同時書き込み。
        write(&mut gpio, &mut ctx, 0x10, (1 << (5 + 16)) | (1 << 5));
        assert_eq!(read(&mut gpio, &mut ctx, 0x0C), 1 << 5);
    }

    #[test]
    fn failed_pin_notification_is_retried_on_a_later_access() {
        let mut gpio = GpioF1::new(GpioF1Params);
        let source = NodeId(3);
        let signal = SignalId(0x32);
        gpio.bind_clock(source, signal);
        let mut ctx = TestCtx::failing_once();
        gpio.on_signal(&mut ctx, source, signal, true).unwrap();
        let crl = (CR_RESET & !(0xF << 20)) | (0x3 << 20);

        assert_eq!(
            gpio.write(&mut ctx, 0x00, AccessSize::Word, u64::from(crl), &attr()),
            Err(BusFault::Device)
        );
        assert!(ctx.edges.is_empty());

        // Rewriting the same configuration retries the pending low level even
        // though neither CRL nor ODR changes on this access.
        gpio.write(&mut ctx, 0x00, AccessSize::Word, u64::from(crl), &attr())
            .unwrap();
        assert_eq!(ctx.edges, vec![(pin_signal(5), false)]);
    }

    #[test]
    fn input_and_af_pins_do_not_emit() {
        let (mut gpio, mut ctx, _source, _signal) = clocked();
        // リセット状態 (全ピン浮遊入力) では ODR を書いても信号は出ない。
        write(&mut gpio, &mut ctx, 0x0C, 0xFFFF);
        assert!(ctx.edges.is_empty());
        assert_eq!(read(&mut gpio, &mut ctx, 0x08), 0); // 入力ピンの IDR は 0。
                                                        // PA9 を AF 出力 (CNF=10, MODE=11 → 0xB) に設定。ODR 変化でも出ない。
        let crh = (CR_RESET & !(0xF << 4)) | (0xB << 4);
        write(&mut gpio, &mut ctx, 0x04, crh);
        write(&mut gpio, &mut ctx, 0x10, 1 << 9);
        assert!(ctx.edges.is_empty());
    }

    #[test]
    fn lock_freezes_configuration() {
        let (mut gpio, mut ctx, _source, _signal) = clocked();
        write(&mut gpio, &mut ctx, 0x18, (1 << 16) | 0x1); // LCKK | LCK0
        assert_eq!(read(&mut gpio, &mut ctx, 0x18), (1 << 16) | 0x1);
        write(&mut gpio, &mut ctx, 0x04, 0);
        assert_eq!(read(&mut gpio, &mut ctx, 0x04), CR_RESET);
        write(&mut gpio, &mut ctx, 0x18, 0); // ロック解除も不可。
        assert_eq!(read(&mut gpio, &mut ctx, 0x18), (1 << 16) | 0x1);
        // ODR/BSRR はロック後も動作する。
        write(&mut gpio, &mut ctx, 0x0C, 0xFFFF);
        assert_eq!(read(&mut gpio, &mut ctx, 0x0C), 0xFFFF);
    }
}
