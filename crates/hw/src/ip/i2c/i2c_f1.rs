//! STM32F1 I2C の機能レジスタモデル (RM0008 §27)。
//!
//! カタログ ID `ip:st/i2c-f1`。対象は RM0008 の I2C1/I2C2（ミディアム
//! デンシティ）: CR1/CR2/OAR1/OAR2/DR/SR1/SR2/CCR/TRISE の 9 レジスタと、
//! マスタ送信・マスタ受信・リスタート付き書き込み→読み出しの 3 形態。
//!
//! - **転送の単位**: RM0008 は START/STOP ビットでフレーム境界を指示する。
//!   本モデルは STOP=1（または読み出しフェーズへのリスタート確定時）に
//!   それまでの DR 書き込みを [`I2cInitiator::write`] /
//!   [`I2cInitiator::write_read`] / [`I2cInitiator::read`] 1 トランザクション
//!   として発行する（fabric がトランザクション単位であるため。ADR 0032
//!   の機能近似）。
//! - **マスタ受信のバースト近似**: RM0008 は ACK ビットで 1 バイト単位の
//!   受信完了を制御するが、fabric はトランザクション単位。最初の DR 読み
//!   出しで `rx-burst` バイト（デフォルト 32）を先行受信し、以降の DR 読み
//!   出しに順に供給する。バッファが尽きた DR 読み出しは継続読み出し
//!   （ポインタ再設定なしの追加 `read`）を発行する。通常のドライバは
//!   読み出しのたびにレジスタポインタを書き直すため、先行受信の余剰分が
//!   問題になることはない。
//! - **ステータス**: SB/ADDR/BTF/TxE/RxNE/STOPF と SR2 の MSL/BUSY/TRA を
//!   RM0008 の状態遷移に従って更新する。ADDR は「SR1 を読んでから SR2 を
//!   読む」でクリア、STOPF は CR1 への任意の書き込みでクリア。
//! - **割り込み**: EV 出力 = (SB|ADDR|ADD10|STOPF|BTF)&ITEVTEN |
//!   (TxE|RxNE)&ITBUFEN、ER 出力 = (BERR|ARLO|AF|OVR|PECERR|TIMEOUT|
//!   SMBALERT)&ITERREN（RM0008 表 85/86）。
//! - **クロックゲーティング**: RCC の APB1ENR.I2C1EN 入力
//!   ([`I2cF1::bind_clock`]) が閉じている間は読み出し 0・書き込み破棄。
//!
//! 未実装の境界: 10 ビットアドレッシング (ADD10)、スレーブモード、
//! デュアルアドレス、SMBus、PEC、DMA、クロックストレッチ制御 (NOSTRETCH)、
//! ARP。予約ビットは raz-wi。

use mosaic_contract::i2c::{I2cAddr, I2cError, I2cInitiator};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, Fnv64, NodeId, SignalId, Snapshot, SnapshotError};

/// レジスタオフセット (RM0008 §27.4)。マップ全体を文書として保持する。
mod reg {
    #![allow(dead_code)]

    pub const CR1: u64 = 0x00;
    pub const CR2: u64 = 0x04;
    pub const OAR1: u64 = 0x08;
    pub const OAR2: u64 = 0x0C;
    pub const DR: u64 = 0x10;
    pub const SR1: u64 = 0x14;
    pub const SR2: u64 = 0x18;
    pub const CCR: u64 = 0x1C;
    pub const TRISE: u64 = 0x20;
    /// TRISE を含むマップ幅。
    pub const SPAN: u64 = 0x24;
}

/// CR1 ビット。データシートの全ビットを文書として保持する。
mod cr1 {
    #![allow(dead_code)]

    pub const PE: u32 = 1 << 0;
    pub const SMBUS: u32 = 1 << 1;
    pub const SMBTYPE: u32 = 1 << 3;
    pub const ENARP: u32 = 1 << 4;
    pub const ENPEC: u32 = 1 << 5;
    pub const ENGC: u32 = 1 << 6;
    pub const NOSTRETCH: u32 = 1 << 7;
    pub const START: u32 = 1 << 8;
    pub const STOP: u32 = 1 << 9;
    pub const ACK: u32 = 1 << 10;
    pub const POS: u32 = 1 << 11;
    pub const PEC: u32 = 1 << 12;
    pub const ALERT: u32 = 1 << 13;
    pub const SWRST: u32 = 1 << 15;
    pub const MASK: u32 = 0xBF7F;
}

/// CR2 ビット。
mod cr2 {
    #![allow(dead_code)]

    pub const ITERREN: u32 = 1 << 1;
    pub const ITEVTEN: u32 = 1 << 2;
    pub const ITBUFEN: u32 = 1 << 3;
    pub const MASK: u32 = 0x3F | ITERREN | ITEVTEN | ITBUFEN;
}

/// SR1 ビット。
mod sr1 {
    #![allow(dead_code)]

    pub const SB: u32 = 1 << 0;
    pub const ADDR: u32 = 1 << 1;
    pub const BTF: u32 = 1 << 2;
    pub const ADD10: u32 = 1 << 3;
    pub const STOPF: u32 = 1 << 4;
    pub const RXNE: u32 = 1 << 6;
    pub const TXE: u32 = 1 << 7;
    pub const BERR: u32 = 1 << 8;
    pub const ARLO: u32 = 1 << 9;
    pub const AF: u32 = 1 << 10;
    pub const OVR: u32 = 1 << 11;
    pub const PECERR: u32 = 1 << 12;
    pub const TIMEOUT: u32 = 1 << 14;
    pub const SMBALERT: u32 = 1 << 15;
}

/// SR2 ビット。
mod sr2 {
    #![allow(dead_code)]

    pub const MSL: u32 = 1 << 0;
    pub const BUSY: u32 = 1 << 1;
    pub const TRA: u32 = 1 << 2;
    pub const GENCALL: u32 = 1 << 4;
}

/// マスタ転送のフェーズ。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// バスアイドル (BUSY=0)。START で待ち受けへ。
    Idle,
    /// START=1 受理済み。次の DR 書き込みはアドレスバイト。
    AwaitAddress,
    /// アドレス (W) 受理済み。DR 書き込みで送信データを蓄積。
    Transmitter,
    /// アドレス (R) 受理済み・まだ DR を読んでいない。最初の読み出しで
    /// fabric から受信する。
    ReceiverPending { restart: bool },
    /// 受信バッファにデータがある（または継続読み出しで補充可能）。
    Receiver,
}

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct I2cF1Params {
    /// EV 割り込み出力 (I2C1_EV, RM0008 NVIC 位置 31)。
    pub ev_irq: SignalId,
    /// ER 割り込み出力 (I2C1_ER, RM0008 NVIC 位置 32)。
    pub er_irq: SignalId,
    /// マスタ受信の先行読み出し量。機能近似のパラメータ。
    pub rx_burst: usize,
}

impl I2cF1Params {
    pub fn new(ev_irq: SignalId, er_irq: SignalId) -> Self {
        Self {
            ev_irq,
            er_irq,
            rx_burst: 32,
        }
    }
}

/// STM32F1 I2C の機能モデル。
pub struct I2cF1 {
    params: I2cF1Params,
    initiator: Box<dyn I2cInitiator>,
    clock_input: Option<(NodeId, SignalId)>,
    cr1: u32,
    cr2: u32,
    oar1: u32,
    oar2: u32,
    sr1: u32,
    sr2: u32,
    ccr: u32,
    trise: u32,
    phase: Phase,
    /// 現在のトランザクションの宛先 (7 ビット)。
    addr: u8,
    /// 送信フェーズで蓄積したバイト (STOP またはリスタートでフラッシュ)。
    tx: Vec<u8>,
    /// 受信バッファ。
    rx: std::collections::VecDeque<u8>,
    /// ADDR クリア手順の「SR1 を読んだ」フラグ (RM0008 §27.4.4)。
    sr1_read: bool,
    clock_on: bool,
    ev_level: bool,
    er_level: bool,
}

impl I2cF1 {
    pub fn new(params: I2cF1Params, initiator: Box<dyn I2cInitiator>) -> Self {
        Self {
            params,
            initiator,
            clock_input: None,
            cr1: 0,
            cr2: 0,
            oar1: 0,
            oar2: 0,
            sr1: sr1::TXE,
            sr2: 0,
            ccr: 0,
            trise: 0,
            phase: Phase::Idle,
            addr: 0,
            tx: Vec::new(),
            rx: Default::default(),
            sr1_read: false,
            clock_on: false,
            ev_level: false,
            er_level: false,
        }
    }

    /// RCC のクロックゲート入力を配線する。
    pub fn bind_clock(&mut self, source: NodeId, signal: SignalId) {
        self.clock_input = Some((source, signal));
    }

    fn enabled(&self) -> bool {
        self.cr1 & cr1::PE != 0
    }

    fn update_irqs(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let ev = (self.sr1 & (sr1::SB | sr1::ADDR | sr1::ADD10 | sr1::STOPF | sr1::BTF) != 0
            && self.cr2 & cr2::ITEVTEN != 0)
            || (self.sr1 & (sr1::TXE | sr1::RXNE) != 0 && self.cr2 & cr2::ITBUFEN != 0);
        if ev != self.ev_level {
            set_optional_signal(ctx, self.params.ev_irq, ev)?;
            self.ev_level = ev;
        }
        let er = self.sr1
            & (sr1::BERR
                | sr1::ARLO
                | sr1::AF
                | sr1::OVR
                | sr1::PECERR
                | sr1::TIMEOUT
                | sr1::SMBALERT)
            != 0
            && self.cr2 & cr2::ITERREN != 0;
        if er != self.er_level {
            set_optional_signal(ctx, self.params.er_irq, er)?;
            self.er_level = er;
        }
        Ok(())
    }

    /// チェックポイント用のフェーズコード。
    fn phase_code(&self) -> u8 {
        match self.phase {
            Phase::Idle => 0,
            Phase::AwaitAddress => 1,
            Phase::Transmitter => 2,
            Phase::ReceiverPending { restart: false } => 3,
            Phase::ReceiverPending { restart: true } => 4,
            Phase::Receiver => 5,
        }
    }

    /// SR1/SR2 のマスタビットをフェーズから導出する。
    fn refresh_master_flags(&mut self) {
        match self.phase {
            Phase::Idle => self.sr2 &= !(sr2::MSL | sr2::BUSY | sr2::TRA),
            Phase::AwaitAddress => self.sr2 |= sr2::MSL | sr2::BUSY, // 方向未確定
            Phase::Transmitter => self.sr2 |= sr2::MSL | sr2::BUSY | sr2::TRA,
            Phase::ReceiverPending { .. } | Phase::Receiver => {
                self.sr2 |= sr2::MSL | sr2::BUSY;
                self.sr2 &= !sr2::TRA;
            }
        }
    }

    fn enter_idle(&mut self) {
        self.phase = Phase::Idle;
        self.tx.clear();
        self.rx.clear();
        self.sr1 &= !(sr1::SB | sr1::ADDR | sr1::BTF | sr1::RXNE | sr1::ADD10);
        self.sr1 |= sr1::TXE;
        self.refresh_master_flags();
    }

    /// CR1 の START/STOP/PE/SWRST 副作用を処理する。
    fn write_cr1(&mut self, ctx: &mut dyn Ctx, value: u32) -> Result<(), BusFault> {
        if value & cr1::SWRST != 0 {
            // RM0008 §27.4.1: SWRST は周辺をリセットする。イニシエータ面と
            // クロック配線はリセットをまたいで保持する。
            let params = self.params;
            let clock_input = self.clock_input;
            let clock_on = self.clock_on;
            let initiator = core::mem::replace(&mut self.initiator, Box::new(NoInitiator));
            *self = I2cF1::new(params, initiator);
            self.clock_input = clock_input;
            self.clock_on = clock_on;
            self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
            return Ok(());
        }
        let rising_start = value & cr1::START != 0 && self.cr1 & cr1::START == 0;
        let stopping = value & cr1::STOP != 0;
        let disabling = value & cr1::PE == 0 && self.cr1 & cr1::PE != 0;
        self.cr1 = value & cr1::MASK;
        // STOPF は CR1 への任意の書き込みでクリアされる (RM0008 §27.4.5)。
        self.sr1 &= !sr1::STOPF;
        if disabling {
            self.enter_idle();
            self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
            return Ok(());
        }
        if !self.enabled() {
            return Ok(());
        }
        if stopping {
            self.flush_transaction(ctx)?;
        }
        if rising_start {
            // EV5: SB セット、次の DR 書き込みはアドレスバイト。送信バイトが
            // 残っている START はリスタート: 書き込み相は維持される。
            self.rx.clear();
            self.sr1 &= !(sr1::ADDR | sr1::RXNE | sr1::ADD10 | sr1::TXE | sr1::BTF);
            self.sr1 |= sr1::SB;
            self.phase = Phase::AwaitAddress;
            self.refresh_master_flags();
        }
        self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
        Ok(())
    }

    /// STOP（または PE=0）で確定したトランザクションを fabric へ発行する。
    fn flush_transaction(&mut self, ctx: &mut dyn Ctx) -> Result<(), BusFault> {
        match self.phase {
            Phase::Transmitter => {
                let result = self.initiator.write(ctx, self.target(), &self.tx);
                self.finish_write(result);
            }
            Phase::ReceiverPending { restart: true } => {
                // 送信後にリスタートしたが読み出さずに STOP: ポインタ書き込み
                // のみのトランザクションとして発行する。
                let result = self.initiator.write(ctx, self.target(), &self.tx);
                self.finish_write(result);
            }
            _ => {}
        }
        self.enter_idle();
        self.sr1 |= sr1::STOPF;
        Ok(())
    }

    fn finish_write(&mut self, result: Result<(), I2cError>) {
        self.tx.clear();
        if result.is_err() {
            self.sr1 |= sr1::AF;
        }
    }

    fn target(&self) -> I2cAddr {
        // `addr` is validated when the address byte is written; the reserve
        // pool (0x00..0x07 / 0x78..0x7F) never survives the write path.
        I2cAddr::seven(self.addr).unwrap_or(I2cAddr::seven(0x08).unwrap())
    }

    /// DR への書き込み: アドレスフェーズでは宛先、送信フェーズではデータ。
    fn write_dr(&mut self, ctx: &mut dyn Ctx, value: u32) -> Result<(), BusFault> {
        let byte = (value & 0xFF) as u8;
        match self.phase {
            Phase::AwaitAddress if !self.enabled() => Ok(()),
            Phase::AwaitAddress => {
                // START ビットはハードウェアが START 送出時にクリアする
                // (機能近似: これにより ACK 等だけを書き換える CR1 書き込みが
                // 誤って再 START を起こさない)。
                self.cr1 &= !cr1::START;
                self.sr1 &= !sr1::SB;
                if I2cAddr::seven(byte >> 1).is_err() {
                    // 予約アドレスへの START はアービトレーション外れ相当。
                    self.sr1 |= sr1::AF;
                    self.enter_idle();
                    self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
                    return Ok(());
                }
                self.addr = byte >> 1;
                let had_tx = !self.tx.is_empty();
                if byte & 1 == 0 {
                    // 送信: EV6 の後 EV8_1 (TxE=1、DR 待ち)。リスタート後の
                    // 送信アドレスは同一フレームの続きとして扱う。
                    self.phase = Phase::Transmitter;
                    if !had_tx {
                        self.tx.clear();
                    }
                    self.sr1 |= sr1::ADDR | sr1::TXE;
                } else {
                    // 受信: アドレスは保持、データは DR 読み出しで受信する。
                    // 書き込みバイトが残っていれば write_read になる。
                    self.phase = Phase::ReceiverPending { restart: had_tx };
                    self.sr1 |= sr1::ADDR;
                }
                self.refresh_master_flags();
                self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
                Ok(())
            }
            Phase::Transmitter => {
                // EV8: 書き込んだバイトは「シフト完了」まで進んだものとして
                // TxE/BTF を立てる (タイミングなしの機能近似)。
                self.tx.push(byte);
                self.sr1 |= sr1::TXE | sr1::BTF;
                self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
                Ok(())
            }
            _ => {
                // アイドル中や受信中の DR 書き込みは RM0008 上意味を持たない。
                Ok(())
            }
        }
    }

    /// DR からの読み出し: 受信データを 1 バイト供給する。
    fn read_dr(&mut self, ctx: &mut dyn Ctx) -> u32 {
        match self.phase {
            Phase::ReceiverPending { restart } => {
                self.phase = Phase::Receiver;
                self.pull_rx(ctx, restart);
            }
            Phase::Receiver => {
                if self.rx.is_empty() {
                    // 継続読み出し: ポインタを再設定せず追加受信する。
                    self.pull_rx(ctx, false);
                }
            }
            _ => {}
        }
        let byte = self.rx.pop_front().unwrap_or(0);
        if self.rx.is_empty() {
            self.sr1 &= !(sr1::RXNE | sr1::BTF);
        }
        u32::from(byte)
    }

    /// fabric から `rx_burst` バイトを受信してバッファへ積む。
    fn pull_rx(&mut self, ctx: &mut dyn Ctx, restart: bool) {
        let mut buf = vec![0u8; self.params.rx_burst.max(1)];
        let result = if restart {
            let pending = core::mem::take(&mut self.tx);
            self.initiator
                .write_read(ctx, self.target(), &pending, &mut buf)
        } else {
            self.initiator.read(ctx, self.target(), &mut buf)
        };
        match result {
            Ok(()) => {
                self.rx.extend(buf);
                self.sr1 |= sr1::RXNE | sr1::BTF;
            }
            Err(_) => {
                self.sr1 |= sr1::AF;
            }
        }
    }
}

/// SWRST 再構築のためのダミー (即座に本来のイニシエータへ差し替える)。
struct NoInitiator;
impl I2cInitiator for NoInitiator {
    fn set_speed(&mut self, _speed: mosaic_contract::i2c::I2cSpeed) {}
    fn write(&mut self, _ctx: &mut dyn Ctx, _addr: I2cAddr, _bytes: &[u8]) -> Result<(), I2cError> {
        Err(I2cError::BusBusy)
    }
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        _addr: I2cAddr,
        _buf: &mut [u8],
    ) -> Result<(), I2cError> {
        Err(I2cError::BusBusy)
    }
    fn write_read(
        &mut self,
        _ctx: &mut dyn Ctx,
        _addr: I2cAddr,
        _out: &[u8],
        _input: &mut [u8],
    ) -> Result<(), I2cError> {
        Err(I2cError::BusBusy)
    }
}

impl MmioTarget for I2cF1 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if !self.clock_on {
            return Ok(0);
        }
        let value = match offset {
            reg::CR1 => self.cr1,
            reg::CR2 => self.cr2,
            reg::OAR1 => self.oar1,
            reg::OAR2 => self.oar2,
            // DR: 受信の開始/供給という副作用を持つ (RM0008 §27.4.6)。
            reg::DR => {
                let value = self.read_dr(ctx);
                self.update_irqs(ctx).map_err(|_| BusFault::Device)?;
                value
            }
            reg::SR1 => {
                self.sr1_read = true;
                self.sr1
            }
            reg::SR2 => {
                if self.sr1_read {
                    // RM0008 §27.4.4: SR1 読み出し後の SR2 読み出しで ADDR を
                    // クリアする。マスタ受信ではこの時点で第 1 バイトの受信が
                    // 進行中になる (§27.3.2 の EV7 タイミング)。
                    self.sr1 &= !sr1::ADDR;
                    self.sr1_read = false;
                    if let Phase::ReceiverPending { restart } = self.phase {
                        self.phase = Phase::Receiver;
                        self.pull_rx(ctx, restart);
                    }
                }
                self.sr2
            }
            reg::CCR => self.ccr,
            reg::TRISE => self.trise,
            _ => 0,
        };
        let bytes = size.bytes() as usize;
        Ok(u64::from(value) & ((1u64 << (bytes * 8)) - 1))
    }

    fn write(
        &mut self,
        _ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if !self.clock_on {
            return Ok(());
        }
        let mask = match size.bytes() {
            1 => 0xFF,
            2 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        let value = (value & mask) as u32;
        match offset {
            reg::CR1 => self.write_cr1(_ctx, value)?,
            reg::CR2 => self.cr2 = value & cr2::MASK,
            reg::OAR1 => self.oar1 = value,
            reg::OAR2 => self.oar2 = value,
            reg::DR => self.write_dr(_ctx, value)?,
            reg::SR1 | reg::SR2 => {}
            reg::CCR => self.ccr = value,
            reg::TRISE => self.trise = value,
            _ => {}
        }
        Ok(())
    }
}

impl EventTarget for I2cF1 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        _source: NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        if let Some((_, gate)) = self.clock_input {
            if signal == gate {
                self.clock_on = high;
            }
        }
        Ok(())
    }
}

/// チェックポイント状態 (ADR 0011 の型付き State)。
#[derive(Clone, Debug)]
pub struct I2cF1Checkpoint {
    pub schema: u32,
    pub cr1: u32,
    pub cr2: u32,
    pub oar1: u32,
    pub oar2: u32,
    pub sr1: u32,
    pub sr2: u32,
    pub ccr: u32,
    pub trise: u32,
    pub addr: u8,
    pub tx: Vec<u8>,
    pub rx: Vec<u8>,
    pub clock_on: bool,
    pub phase: u8,
    pub sr1_read: bool,
    pub ev_level: bool,
    pub er_level: bool,
}

impl Snapshot for I2cF1 {
    type State = I2cF1Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        I2cF1Checkpoint {
            schema: Self::SCHEMA_VERSION,
            cr1: self.cr1,
            cr2: self.cr2,
            oar1: self.oar1,
            oar2: self.oar2,
            sr1: self.sr1,
            sr2: self.sr2,
            ccr: self.ccr,
            trise: self.trise,
            addr: self.addr,
            tx: self.tx.clone(),
            rx: self.rx.iter().copied().collect(),
            clock_on: self.clock_on,
            phase: self.phase_code(),
            sr1_read: self.sr1_read,
            ev_level: self.ev_level,
            er_level: self.er_level,
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.phase > 5 {
            return Err(SnapshotError::InvalidState);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.cr1 = state.cr1;
        self.cr2 = state.cr2;
        self.oar1 = state.oar1;
        self.oar2 = state.oar2;
        self.sr1 = state.sr1;
        self.sr2 = state.sr2;
        self.ccr = state.ccr;
        self.trise = state.trise;
        self.addr = state.addr;
        self.tx = state.tx.clone();
        self.rx = state.rx.iter().copied().collect();
        self.clock_on = state.clock_on;
        self.sr1_read = state.sr1_read;
        self.ev_level = state.ev_level;
        self.er_level = state.er_level;
        self.phase = match state.phase {
            0 => Phase::Idle,
            1 => Phase::AwaitAddress,
            2 => Phase::Transmitter,
            3 => Phase::ReceiverPending { restart: false },
            4 => Phase::ReceiverPending { restart: true },
            _ => Phase::Receiver,
        };
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        for word in [
            state.cr1,
            state.cr2,
            state.sr1,
            state.sr2,
            state.oar1,
            state.oar2,
            state.ccr,
            state.trise,
        ] {
            hash.u32(word);
        }
        hash.byte(state.addr);
        for &byte in &state.tx {
            hash.byte(byte);
        }
        for &byte in &state.rx {
            hash.byte(byte);
        }
        hash.bool(state.clock_on);
        hash.byte(state.phase);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::testutil::TestCtx;
    use mosaic_bus::i2c::core::I2cBus;
    use mosaic_contract::i2c::{Ack, I2cTarget};

    struct Logger {
        addr: I2cAddr,
        writes: Mutex<Vec<Vec<u8>>>,
        reads: Mutex<Vec<usize>>,
    }

    impl I2cTarget for Logger {
        fn matches(&self, addr: &I2cAddr) -> bool {
            addr == &self.addr
        }
        fn target_write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            bytes: &[u8],
        ) -> Result<Ack, I2cError> {
            self.writes.lock().unwrap().push(bytes.to_vec());
            Ok(Ack::Ack)
        }
        fn target_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _addr: &I2cAddr,
            buf: &mut [u8],
        ) -> Result<Ack, I2cError> {
            self.reads.lock().unwrap().push(buf.len());
            for (slot, index) in buf.iter_mut().zip(0..) {
                *slot = index as u8;
            }
            Ok(Ack::Ack)
        }
    }

    fn device() -> I2cF1 {
        let bus = I2cBus::new().shared();
        let logger = Logger {
            addr: I2cAddr::seven(0x20).unwrap(),
            writes: Mutex::new(Vec::new()),
            reads: Mutex::new(Vec::new()),
        };
        bus.lock().unwrap().add_target(Box::new(logger));
        let mut device = I2cF1::new(
            I2cF1Params::new(SignalId(1), SignalId(2)),
            Box::new(SharedBus(bus)),
        );
        device.clock_on = true;
        device
    }

    /// [`crate::ip::i2c::catalog`] の SharedI2c と同じ共有バスアダプタ。
    struct SharedBus(Arc<Mutex<I2cBus>>);
    impl I2cInitiator for SharedBus {
        fn set_speed(&mut self, _speed: mosaic_contract::i2c::I2cSpeed) {}
        fn write(
            &mut self,
            ctx: &mut dyn Ctx,
            addr: I2cAddr,
            bytes: &[u8],
        ) -> Result<(), I2cError> {
            self.0.lock().unwrap().write(ctx, addr, bytes)
        }
        fn read(
            &mut self,
            ctx: &mut dyn Ctx,
            addr: I2cAddr,
            buf: &mut [u8],
        ) -> Result<(), I2cError> {
            self.0.lock().unwrap().read(ctx, addr, buf)
        }
        fn write_read(
            &mut self,
            ctx: &mut dyn Ctx,
            addr: I2cAddr,
            out: &[u8],
            input: &mut [u8],
        ) -> Result<(), I2cError> {
            self.0.lock().unwrap().write_read(ctx, addr, out, input)
        }
    }

    fn wr(device: &mut I2cF1, ctx: &mut dyn Ctx, offset: u64, value: u32) {
        device
            .write(
                ctx,
                offset,
                AccessSize::Word,
                u64::from(value),
                &test_attr(),
            )
            .expect("mmio write");
    }

    fn rd(device: &mut I2cF1, ctx: &mut dyn Ctx, offset: u64) -> u32 {
        device
            .read(ctx, offset, AccessSize::Word, &test_attr())
            .expect("mmio read") as u32
    }

    fn test_attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: mosaic_exec::Security::Secure,
            privileged: true,
            kind: mosaic_exec::AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: mosaic_exec::Endian::Little,
        }
    }

    /// SR1 → SR2 の読み出しで ADDR をクリアする (RM0008 §27.4.4)。
    fn clear_addr(device: &mut I2cF1, ctx: &mut dyn Ctx) {
        rd(device, ctx, reg::SR1);
        rd(device, ctx, reg::SR2);
        assert_eq!(rd(device, ctx, reg::SR1) & sr1::ADDR, 0);
    }

    #[test]
    fn master_transmitter_sequence_matches_rm0008() {
        let mut device = device();
        let mut ctx = TestCtx::default();
        // PE=1、EV/BUF 割り込み許可。
        wr(
            &mut device,
            &mut ctx,
            reg::CR2,
            cr2::ITEVTEN | cr2::ITBUFEN | 8,
        );
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE);
        // EV5: START。
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::START);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::SB, sr1::SB);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR2) & sr2::MSL, sr2::MSL);
        // アドレス 0x20 << 1 | W。
        wr(&mut device, &mut ctx, reg::DR, 0x20 << 1);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::ADDR, sr1::ADDR);
        clear_addr(&mut device, &mut ctx);
        // EV8_1/EV8: データ 2 バイト。
        wr(&mut device, &mut ctx, reg::DR, 0x00); // ポインタ
        wr(&mut device, &mut ctx, reg::DR, 0xA5); // データ
                                                  // STOP。
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::STOP);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::STOPF, sr1::STOPF);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR2) & sr2::BUSY, 0);
    }

    #[test]
    fn master_receiver_with_repeated_start_reads_data() {
        let mut device = device();
        let mut ctx = TestCtx::default();
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::ACK);
        wr(
            &mut device,
            &mut ctx,
            reg::CR1,
            cr1::PE | cr1::ACK | cr1::START,
        );
        wr(&mut device, &mut ctx, reg::DR, 0x20 << 1); // W: ポインタ書き込み
        clear_addr(&mut device, &mut ctx);
        wr(&mut device, &mut ctx, reg::DR, 0x12);
        // リスタート → 受信アドレス。
        wr(
            &mut device,
            &mut ctx,
            reg::CR1,
            cr1::PE | cr1::ACK | cr1::START,
        );
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::SB, sr1::SB);
        wr(&mut device, &mut ctx, reg::DR, (0x20 << 1) | 1);
        clear_addr(&mut device, &mut ctx);
        // EV7: DR 読み出し (受信はここで開始される)。
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::RXNE, sr1::RXNE);
        let first = rd(&mut device, &mut ctx, reg::DR);
        let second = rd(&mut device, &mut ctx, reg::DR);
        assert_eq!((first, second), (0, 1));
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::STOP);
    }

    #[test]
    fn clock_gate_makes_the_block_dead() {
        let mut device = device();
        let mut ctx = TestCtx::default();
        device.clock_on = false;
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE);
        assert_eq!(rd(&mut device, &mut ctx, reg::CR1), 0);
        device.clock_on = true;
        assert_eq!(rd(&mut device, &mut ctx, reg::CR1), 0); // 書き込みは捨てられた
    }

    #[test]
    fn swrst_clears_the_peripheral() {
        let mut device = device();
        let mut ctx = TestCtx::default();
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE);
        wr(&mut device, &mut ctx, reg::CCR, 0x1234);
        wr(&mut device, &mut ctx, reg::CR1, cr1::SWRST);
        assert_eq!(rd(&mut device, &mut ctx, reg::CCR), 0);
        assert_eq!(device.phase, Phase::Idle);
    }

    #[test]
    fn nack_sets_af_and_er_irq() {
        let bus = I2cBus::new().shared(); // 誰も乗っていないバス
        let mut device = I2cF1::new(
            I2cF1Params::new(SignalId(1), SignalId(2)),
            Box::new(SharedBus(bus)),
        );
        device.clock_on = true;
        let mut ctx = TestCtx::default();
        wr(&mut device, &mut ctx, reg::CR2, cr2::ITERREN);
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE);
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::START);
        wr(&mut device, &mut ctx, reg::DR, 0x50 << 1);
        clear_addr(&mut device, &mut ctx);
        wr(&mut device, &mut ctx, reg::DR, 0x00);
        wr(&mut device, &mut ctx, reg::CR1, cr1::PE | cr1::STOP);
        assert_eq!(rd(&mut device, &mut ctx, reg::SR1) & sr1::AF, sr1::AF);
    }
}
