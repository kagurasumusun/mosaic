#![no_std]
#![forbid(unsafe_code)]
//! ARM PrimeCell GPIO ポート PL061 の機能レジスタモデル（構成設計書 P2 /
//! ADR 0008、ARM DDI 0190）。
//!
//! 8 本のピンは構築時に [`GpioPort`] として注入する（ポート注入）。
//! ゲストは実機と同じ作法で駆動する:
//! - `GPIODATA` はマスク付きアクセス: アドレス bit[9:2] がピンマスクで、
//!   書き込みはマスクされたピンだけを更新し、読み出しはマスクされたピン
//!   の現在レベルを bit[7:0] に返す（未接続ピンは 0）。全ピンアクセスは
//!   オフセット `0x3FC`。
//! - `GPIODIR` の書き込みはピン方向を切り替え、保持している pull/drive
//!   構成を保ったまま注入ポートへ [`GpioPort::configure`] を転送する。
//! - 割り込みは `GPIOIS`（レベル/エッジ）、`GPIOIBE`（両エッジ）、
//!   `GPIOIEV`（イベント種別）、`GPIOIE`（イネーブル）で設定し、
//!   `GPIORIS` / `GPIOMIS` で観測、`GPIOIC` でクリアする。
//! - `GPIOAFSEL` が立ったピンは GPIO から切り離される（機能モデルでは
//!   代替機能の実体を持たないため、DATA 読み出し 0・書き込み無視）。
//!
//! 機能モデルの簡略化（タイミングを持たない宣言的モデル）:
//! - エッジ検出は遅延評価: レジスタアクセスのたびに注入ポートの
//!   [`GpioPort::current_level`] をサンプリングして RIS を更新する。
//!   レベル検出の RIS は常にライブ再計算（IC で落としても条件が
//!   続けば立ち直す、実機と同じ）。
//! - DMA・代替機能の実体は modeled しない。

extern crate alloc;

use alloc::boxed::Box;
use mosaic_contract_gpio::{GpioPort, PinConfig, PinDirection, PinLevel, Pull};
use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId};
use mosaic_ip_arm_primecell_id::PrimeCellId;

/// レジスタオフセット（GPIODATA は 0x000..0x3FC のウィンドウ）。
mod reg {
    pub const DIR: u64 = 0x400;
    pub const IS: u64 = 0x404;
    pub const IBE: u64 = 0x408;
    pub const IEV: u64 = 0x40C;
    pub const IE: u64 = 0x410;
    pub const RIS: u64 = 0x414;
    pub const MIS: u64 = 0x418;
    pub const IC: u64 = 0x41C;
    pub const AFSEL: u64 = 0x420;
}

/// ピン数。
pub const PINS: usize = 8;

/// 構築・操作エラー。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Pl061Error {
    /// ピン番号が 0..8 を超えた。
    PinOutOfRange,
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
pub struct Pl061Params {
    /// combined 割り込み線。
    pub combined_irq: SignalId,
    /// 未マップオフセットの扱い。
    pub unmapped: UnmappedPolicy,
}

impl Pl061Params {
    pub const fn new(combined_irq: SignalId) -> Self {
        Self {
            combined_irq,
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// スナップショット対象のスカラーレジスタ群（設計書 §10 の state 分離）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pl061State {
    pub schema: u32,
    pub dir: u8,
    pub data_latch: u8,
    pub is: u8,
    pub ibe: u8,
    pub iev: u8,
    pub ie: u8,
    pub ris: u8,
    pub afsel: u8,
    pub last_levels: u8,
    pub irq_level: bool,
}

impl Default for Pl061State {
    fn default() -> Self {
        Self {
            schema: 1,
            dir: 0,
            data_latch: 0,
            is: 0,
            ibe: 0,
            iev: 0,
            ie: 0,
            ris: 0,
            afsel: 0,
            last_levels: 0,
            irq_level: false,
        }
    }
}

/// 機能 PL061。ピンポートは構築後に [`Pl061::attach_pin`] で注入する。
pub struct Pl061 {
    params: Pl061Params,
    state: Pl061State,
    pins: [Option<Box<dyn GpioPort>>; PINS],
    configs: [PinConfig; PINS],
    identity: PrimeCellId,
}

impl Pl061 {
    /// PL061 part number 0x061、revision 0、patch 0。
    pub const PART_NUMBER: u16 = 0x061;

    pub fn new(params: Pl061Params) -> Self {
        Self {
            params,
            state: Pl061State::default(),
            pins: core::array::from_fn(|_| None),
            configs: [PinConfig::input(Pull::None); PINS],
            identity: PrimeCellId::arm(Self::PART_NUMBER, 0, 0)
                .expect("PL061 part number fits the ID fields"),
        }
    }

    /// ピンにポートを接続する。接続時点のレベルをエッジ検出の基準に
    /// 採用する（リセット直後のスパurious エッジを防ぐ）。
    pub fn attach_pin(&mut self, pin: u8, port: Box<dyn GpioPort>) -> Result<(), Pl061Error> {
        if usize::from(pin) >= PINS {
            return Err(Pl061Error::PinOutOfRange);
        }
        if port.current_level() == PinLevel::High {
            self.state.last_levels |= 1 << pin;
        }
        self.pins[usize::from(pin)] = Some(port);
        Ok(())
    }

    /// レベルをサンプリングして RIS を更新する（遅延エッジ検出）。
    fn sample_interrupts(&mut self) {
        for pin in 0..PINS {
            let Some(port) = self.pins[pin].as_ref() else {
                continue;
            };
            let bit = 1u8 << pin;
            let high = port.current_level() == PinLevel::High;
            let was_high = self.state.last_levels & bit != 0;
            if high != was_high {
                if high {
                    self.state.last_levels |= bit;
                } else {
                    self.state.last_levels &= !bit;
                }
                let edge_sensed = self.state.is & bit == 0;
                if edge_sensed {
                    let both = self.state.ibe & bit != 0;
                    let event_high = self.state.iev & bit != 0;
                    if both || high == event_high {
                        self.state.ris |= bit;
                    }
                }
            }
            if self.state.is & bit != 0 {
                // レベル検出: ライブ再計算。
                let event_high = self.state.iev & bit != 0;
                if high == event_high {
                    self.state.ris |= bit;
                } else {
                    self.state.ris &= !bit;
                }
            }
        }
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) {
        self.sample_interrupts();
        let asserted = self.state.ris & self.state.ie != 0;
        if asserted != self.state.irq_level {
            self.state.irq_level = asserted;
            let _ = ctx.set_signal(self.params.combined_irq, asserted);
        }
    }

    fn data_read(&self, mask: u8) -> u32 {
        let mut value = 0u8;
        for pin in 0..PINS {
            let bit = 1u8 << pin;
            if mask & bit == 0 || self.state.afsel & bit != 0 {
                continue;
            }
            let Some(port) = self.pins[pin].as_ref() else {
                continue; // 未接続ピンは 0 を読む。
            };
            if port.current_level() == PinLevel::High {
                value |= bit;
            }
        }
        u32::from(value)
    }

    fn data_write(&mut self, ctx: &mut dyn Ctx, mask: u8, value: u32) {
        let byte = (value & 0xFF) as u8;
        self.state.data_latch = (self.state.data_latch & !mask) | (byte & mask);
        for pin in 0..PINS {
            let bit = 1u8 << pin;
            if mask & bit == 0 || self.state.afsel & bit != 0 {
                continue;
            }
            let Some(port) = self.pins[pin].as_mut() else {
                continue;
            };
            if self.state.dir & bit == 0 {
                // 入力ピン: ラッチは更新されるが線は駆動しない。
                continue;
            }
            let level = if self.state.data_latch & bit != 0 {
                PinLevel::High
            } else {
                PinLevel::Low
            };
            // 駆動失敗（Disconnected 等）は握りつぶさないが、レジスタ
            // アクセス自体は成功させる（機能モデルの方針）。
            let _ = port.drive(ctx, level);
        }
    }

    fn apply_dir(&mut self, value: u32) {
        self.state.dir = (value & 0xFF) as u8;
        for pin in 0..PINS {
            let bit = 1u8 << pin;
            if self.state.afsel & bit != 0 {
                continue; // AF ピンは GPIO から切り離されている。
            }
            let Some(port) = self.pins[pin].as_mut() else {
                continue;
            };
            self.configs[pin].direction = if self.state.dir & bit != 0 {
                PinDirection::Output
            } else {
                PinDirection::Input
            };
            self.configs[pin].initial = if self.state.data_latch & bit != 0 {
                PinLevel::High
            } else {
                PinLevel::Low
            };
            let _ = port.configure(self.configs[pin]);
        }
    }

    fn read_word(&mut self, offset: u64) -> Result<u32, BusFault> {
        if offset < reg::DIR {
            // GPIODATA ウィンドウ: bit[9:2] がピンマスク。
            let mask = ((offset >> 2) & 0xFF) as u8;
            return Ok(self.data_read(mask));
        }
        let value = match offset {
            reg::DIR => u32::from(self.state.dir),
            reg::IS => u32::from(self.state.is),
            reg::IBE => u32::from(self.state.ibe),
            reg::IEV => u32::from(self.state.iev),
            reg::IE => u32::from(self.state.ie),
            // 遅延エッジ検出: 割り込み状態の読み出しは必ずサンプリング後。
            reg::RIS => {
                self.sample_interrupts();
                u32::from(self.state.ris)
            }
            reg::MIS => {
                self.sample_interrupts();
                u32::from(self.state.ris & self.state.ie)
            }
            reg::IC => 0,
            reg::AFSEL => u32::from(self.state.afsel),
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
        if offset < reg::DIR {
            let mask = ((offset >> 2) & 0xFF) as u8;
            self.data_write(ctx, mask, value);
            return self.finish_write(ctx);
        }
        match offset {
            reg::DIR => self.apply_dir(value),
            reg::IS => self.state.is = (value & 0xFF) as u8,
            reg::IBE => self.state.ibe = (value & 0xFF) as u8,
            reg::IEV => self.state.iev = (value & 0xFF) as u8,
            reg::IE => self.state.ie = (value & 0xFF) as u8,
            reg::IC => self.state.ris &= !((value & 0xFF) as u8),
            reg::AFSEL => self.state.afsel = (value & 0xFF) as u8,
            // 読み出し専用レジスタへの書き込みは無視。
            reg::RIS | reg::MIS => {}
            _ => {
                if PrimeCellId::covers(offset) {
                    // 識別レジスタは読み出し専用。
                } else {
                    return self.unmapped(offset).map(|_| ());
                }
            }
        }
        self.finish_write(ctx)
    }

    fn finish_write(&mut self, ctx: &mut dyn Ctx) -> Result<(), BusFault> {
        self.update_irq(ctx);
        Ok(())
    }

    fn unmapped(&self, _offset: u64) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Pl061 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(offset)?;
        // 遅延エッジ検出: 読み出しも RIS / 割り込み線を更新する。
        self.update_irq(ctx);
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
            // PL061 レジスタは 32 ビット幅。8 バイト転送は収まらない。
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for Pl061 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
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
    use mosaic_contract_gpio::{DriveMode, GpioError};
    use mosaic_core_types::{AccessKind, Endian, EventId, NodeId, Security, VTime};

    const IRQ: SignalId = SignalId(6);

    struct FakeProbe {
        drives: Vec<PinLevel>,
        configs: Vec<PinConfig>,
        level: PinLevel,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            Self {
                drives: Vec::new(),
                configs: Vec::new(),
                level: PinLevel::Low,
            }
        }
    }

    struct FakePort(Arc<Mutex<FakeProbe>>);

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl FakePort {
        fn new() -> (Self, Arc<Mutex<FakeProbe>>) {
            let probe = Arc::new(Mutex::new(FakeProbe::default()));
            (Self(Arc::clone(&probe)), probe)
        }
    }

    impl GpioPort for FakePort {
        fn drive(&mut self, _ctx: &mut dyn Ctx, level: PinLevel) -> Result<(), GpioError> {
            let mut probe = lock(&self.0);
            probe.drives.push(level);
            probe.level = level;
            Ok(())
        }
        fn configure(&mut self, config: PinConfig) -> Result<(), GpioError> {
            lock(&self.0).configs.push(config);
            Ok(())
        }
        fn current_level(&self) -> PinLevel {
            lock(&self.0).level
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

    struct Bench {
        device: Pl061,
        ctx: TestCtx,
        probes: Vec<Arc<Mutex<FakeProbe>>>,
    }

    fn bench(attached: usize) -> Bench {
        let mut device = Pl061::new(Pl061Params::new(IRQ));
        let mut probes = Vec::new();
        for pin in 0..attached {
            let (port, probe) = FakePort::new();
            device.attach_pin(pin as u8, Box::new(port)).unwrap();
            probes.push(probe);
        }
        Bench {
            device,
            ctx: TestCtx {
                signals: Vec::new(),
            },
            probes,
        }
    }

    fn read(device: &mut Pl061, ctx: &mut TestCtx, offset: u64) -> u32 {
        device.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn write(device: &mut Pl061, ctx: &mut TestCtx, offset: u64, value: u32) {
        device
            .write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn primecell_identification_reads_back() {
        let bench = bench(0);
        let mut device = bench.device;
        let mut ctx = bench.ctx;
        assert_eq!(read(&mut device, &mut ctx, 0xFE0), 0x61);
        assert_eq!(read(&mut device, &mut ctx, 0xFE4), 0x410);
        assert_eq!(read(&mut device, &mut ctx, 0xFF0), 0x0D);
        assert_eq!(read(&mut device, &mut ctx, 0xFFC), 0xB1);
    }

    #[test]
    fn por_registers_are_cleared() {
        let bench = bench(2);
        let mut device = bench.device;
        let mut ctx = bench.ctx;
        for offset in [
            reg::DIR,
            reg::IS,
            reg::IBE,
            reg::IEV,
            reg::IE,
            reg::RIS,
            reg::MIS,
            reg::AFSEL,
        ] {
            assert_eq!(read(&mut device, &mut ctx, offset), 0, "offset {offset:#x}");
        }
    }

    #[test]
    fn masked_data_write_drives_only_selected_pins() {
        let mut bench = bench(2);
        // pin0/pin1 を出力にする。
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0xFF);
        // マスク bit0 だけの書き込み (offset 0x004): pin0 だけ駆動される。
        write(&mut bench.device, &mut bench.ctx, 0x004, 0x01);
        assert_eq!(lock(&bench.probes[0]).drives, vec![PinLevel::High]);
        assert!(lock(&bench.probes[1]).drives.is_empty());
        // マスク bit1 (offset 0x008) に 0: pin1 だけ Low 駆動。
        write(&mut bench.device, &mut bench.ctx, 0x008, 0x00);
        assert_eq!(lock(&bench.probes[1]).drives, vec![PinLevel::Low]);
        assert_eq!(lock(&bench.probes[0]).drives.len(), 1);
    }

    #[test]
    fn full_data_access_reads_and_writes_all_pins() {
        let mut bench = bench(2);
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0xFF);
        write(&mut bench.device, &mut bench.ctx, 0x3FC, 0x02);
        assert_eq!(lock(&bench.probes[0]).drives, vec![PinLevel::Low]);
        assert_eq!(lock(&bench.probes[1]).drives, vec![PinLevel::High]);
        // FakePort は駆動値をレベルに反映するので読み出しは同じ値になる。
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x3FC), 0x02);
        // マスク読み出し: bit0 だけ → pin0 の Low は 0。
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x004), 0x00);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x008), 0x02);
    }

    #[test]
    fn disconnected_pins_read_zero_and_ignore_writes() {
        let mut bench = bench(0);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x3FC), 0x00);
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0xFF);
        write(&mut bench.device, &mut bench.ctx, 0x3FC, 0xFF);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x3FC), 0x00);
    }

    #[test]
    fn input_pins_latch_but_do_not_drive() {
        let mut bench = bench(1);
        // DIR = 0（既定の入力）のままラッチだけ更新する。
        write(&mut bench.device, &mut bench.ctx, 0x3FC, 0x01);
        assert!(lock(&bench.probes[0]).drives.is_empty());
        // 出力に切り替えた後の書き込みで初めて駆動される。
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0x01);
        write(&mut bench.device, &mut bench.ctx, 0x3FC, 0x00);
        assert_eq!(lock(&bench.probes[0]).drives, vec![PinLevel::Low]);
    }

    #[test]
    fn dir_writes_forward_configuration_with_preserved_pull_and_drive() {
        let mut bench = bench(1);
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0x01);
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0x00);
        let configs = lock(&bench.probes[0]).configs.clone();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].direction, PinDirection::Output);
        assert_eq!(configs[1].direction, PinDirection::Input);
        // pull / drive は PL061 が保持する基本構成のまま転送される。
        for config in &configs {
            assert_eq!(config.pull, Pull::None);
            assert_eq!(config.drive, DriveMode::PushPull);
        }
    }

    #[test]
    fn both_edge_interrupts_latch_and_clear() {
        let mut bench = bench(1);
        write(&mut bench.device, &mut bench.ctx, reg::IBE, 0x01);
        write(&mut bench.device, &mut bench.ctx, reg::IE, 0x01);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0);
        // 立ち上がり。
        lock(&bench.probes[0]).level = PinLevel::High;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x01);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::MIS), 0x01);
        assert_eq!(bench.ctx.signals, vec![(IRQ, true)]);
        // クリアして立ち下がり: IBE なので再びラッチされる。
        write(&mut bench.device, &mut bench.ctx, reg::IC, 0x01);
        assert_eq!(bench.ctx.signals, vec![(IRQ, true), (IRQ, false)]);
        lock(&bench.probes[0]).level = PinLevel::Low;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x01);
    }

    #[test]
    fn single_edge_interrupts_respect_iev() {
        let mut bench = bench(1);
        write(&mut bench.device, &mut bench.ctx, reg::IEV, 0x01); // 立ち上がりのみ
        write(&mut bench.device, &mut bench.ctx, reg::IE, 0x01);
        lock(&bench.probes[0]).level = PinLevel::High;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x01);
        write(&mut bench.device, &mut bench.ctx, reg::IC, 0x01);
        lock(&bench.probes[0]).level = PinLevel::Low; // 立ち下がりは無視
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x00);
    }

    #[test]
    fn level_interrupts_are_live_and_reassert_after_clear() {
        let mut bench = bench(1);
        write(&mut bench.device, &mut bench.ctx, reg::IS, 0x01); // レベル検出
        write(&mut bench.device, &mut bench.ctx, reg::IEV, 0x01); // High
        write(&mut bench.device, &mut bench.ctx, reg::IE, 0x01);
        lock(&bench.probes[0]).level = PinLevel::High;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x01);
        // 条件が続く限り IC では落ちない（実機と同じライブ再計算）。
        write(&mut bench.device, &mut bench.ctx, reg::IC, 0x01);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x01);
        // 条件が消えると落ちる。
        lock(&bench.probes[0]).level = PinLevel::Low;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::RIS), 0x00);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, reg::MIS), 0x00);
    }

    #[test]
    fn afsel_detaches_pins_from_gpio() {
        let mut bench = bench(1);
        write(&mut bench.device, &mut bench.ctx, reg::DIR, 0x01);
        write(&mut bench.device, &mut bench.ctx, reg::AFSEL, 0x01);
        // AF ピンへの DATA 書き込みは駆動されない。
        write(&mut bench.device, &mut bench.ctx, 0x3FC, 0x01);
        assert!(lock(&bench.probes[0]).drives.is_empty());
        // AF ピンの読み出しは 0。
        lock(&bench.probes[0]).level = PinLevel::High;
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x3FC), 0x00);
    }

    #[test]
    fn attach_is_bounds_checked() {
        let mut device = Pl061::new(Pl061Params::new(IRQ));
        let (port, _) = FakePort::new();
        assert_eq!(
            device.attach_pin(PINS as u8, Box::new(port)),
            Err(Pl061Error::PinOutOfRange)
        );
    }

    #[test]
    fn unmapped_and_oversized_accesses_follow_policy() {
        let mut bench = bench(0);
        assert_eq!(read(&mut bench.device, &mut bench.ctx, 0x500), 0);
        write(&mut bench.device, &mut bench.ctx, 0x500, 1);
        assert_eq!(
            bench
                .device
                .write(&mut bench.ctx, reg::DIR, AccessSize::DoubleWord, 0, &attr()),
            Err(BusFault::InvalidSize)
        );
        let mut params = Pl061Params::new(IRQ);
        params.unmapped = UnmappedPolicy::Fault;
        let mut strict = Pl061::new(params);
        let mut ctx = TestCtx {
            signals: Vec::new(),
        };
        assert_eq!(
            strict.read(&mut ctx, 0x500, AccessSize::Word, &attr()),
            Err(BusFault::Device)
        );
    }
}
