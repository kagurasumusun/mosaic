#![no_std]
#![forbid(unsafe_code)]
//! Synopsys DesignWare APB I2C コントローラの機能レジスタモデル（マスタ
//! モードのみ、構成設計書 P2 / ADR 0008）。
//!
//! ゲストは実機と同じ作法で駆動する: `IC_ENABLE` で有効化し、`IC_TAR` に
//! 7 ビットアドレスを置き、`IC_DATA_CMD` へコマンド（bit8 = read、
//! bit9 = STOP、bit10 = RESTART）を積む。STOP ビットが積まれた時点で
//! キュー全体が 1 バッチとして実行される:
//!
//! - 書き込みコマンド列 → [`I2cInitiator::write`]
//! - 読み出しコマンド列 → [`I2cInitiator::read`]
//! - 書き込み + RESTART 付き読み出し → [`I2cInitiator::write_read`]
//! - 書き込み + RESTART なし読み出し → write と read の 2 トランザクション
//!
//! 実行結果は即時に RX FIFO へ入り、`IC_STATUS.RFNE` / `IC_RXFLR` から
//! 観測できる。NACK は `R_TX_ABRT` と `IC_TX_ABRT_SOURCE` で報告し、
//! キューを破棄する。
//!
//! 機能モデルの簡略化（タイミングを持たない宣言的モデル）:
//! - SCL 分周レジスタ（HCNT/LCNT）は保存のみでクロックは生成しない。
//! - TX FIFO は深さ `tx_depth` のコマンドキューとして modeled するが、
//!   バス転送は STOP 時に同期的に完了する（RFNE 待ち時間は 0）。
//! - スレーブモード・DMA・10 ビットアドレスは modeled しない。
//! - 読み出しフェーズの後に書き込みコマンドが続くバッチは
//!   `ABRT_TXDATA_NOACK` で中断する（実機の任意 RESTART 連鎖は未 modeled）。
//! - 無効化（`IC_ENABLE = 0`）はキューと RX FIFO を即フラッシュする。

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use mosaic_contract_i2c::{I2cAddr, I2cError, I2cInitiator, I2cSpeed};
use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError};

/// DW_apb_i2c レジスタオフセット（APB_DATA_WIDTH = 32 の構成）。
mod reg {
    pub const CON: u64 = 0x000;
    pub const TAR: u64 = 0x004;
    pub const SAR: u64 = 0x008;
    pub const HS_MADDR: u64 = 0x00C;
    pub const DATA_CMD: u64 = 0x010;
    pub const SS_SCL_HCNT: u64 = 0x014;
    pub const SS_SCL_LCNT: u64 = 0x018;
    pub const FS_SCL_HCNT: u64 = 0x01C;
    pub const FS_SCL_LCNT: u64 = 0x020;
    pub const HS_SCL_HCNT: u64 = 0x024;
    pub const HS_SCL_LCNT: u64 = 0x028;
    pub const INTR_STAT: u64 = 0x02C;
    pub const INTR_MASK: u64 = 0x030;
    pub const RAW_INTR_STAT: u64 = 0x034;
    pub const RX_TL: u64 = 0x038;
    pub const TX_TL: u64 = 0x03C;
    pub const CLR_INTR: u64 = 0x040;
    pub const CLR_RX_UNDER: u64 = 0x044;
    pub const CLR_RX_OVER: u64 = 0x048;
    pub const CLR_TX_OVER: u64 = 0x04C;
    pub const CLR_RD_REQ: u64 = 0x050;
    pub const CLR_TX_ABRT: u64 = 0x054;
    pub const CLR_RX_DONE: u64 = 0x058;
    pub const CLR_ACTIVITY: u64 = 0x05C;
    pub const CLR_STOP_DET: u64 = 0x060;
    pub const CLR_START_DET: u64 = 0x064;
    pub const CLR_GEN_CALL: u64 = 0x068;
    pub const ENABLE: u64 = 0x06C;
    pub const STATUS: u64 = 0x070;
    pub const TXFLR: u64 = 0x074;
    pub const RXFLR: u64 = 0x078;
    pub const SDA_HOLD: u64 = 0x07C;
    pub const TX_ABRT_SOURCE: u64 = 0x080;
    pub const SLV_DATA_NACK_ONLY: u64 = 0x084;
    pub const DMA_CR: u64 = 0x088;
    pub const DMA_TDLR: u64 = 0x08C;
    pub const DMA_RDLR: u64 = 0x090;
    pub const SDA_SETUP: u64 = 0x094;
    pub const ACK_GENERAL_CALL: u64 = 0x098;
    pub const ENABLE_STATUS: u64 = 0x09C;
    pub const COMP_TYPE: u64 = 0x0F8;
    pub const COMP_VERSION: u64 = 0x0FC;
}

/// IC_CON ビット（Linux i2c-designware と同じ配置）。
mod con {
    pub const MASTER_MODE: u32 = 1 << 0;
    pub const SPEED_MASK: u32 = 0b11 << 1;
    pub const SPEED_STANDARD: u32 = 0b01 << 1;
    pub const SPEED_FAST: u32 = 0b10 << 1;
    pub const SPEED_HIGH: u32 = 0b11 << 1;
    pub const RESTART_EN: u32 = 1 << 5;
}

/// IC_DATA_CMD ビット。
mod data_cmd {
    pub const CMD_READ: u32 = 1 << 8;
    pub const STOP: u32 = 1 << 9;
    pub const RESTART: u32 = 1 << 10;
}

/// 生割り込みビット（IC_RAW_INTR_STAT / IC_INTR_MASK）。
mod intr {
    pub const RX_UNDER: u32 = 1 << 0;
    pub const RX_OVER: u32 = 1 << 1;
    pub const RX_FULL: u32 = 1 << 2;
    pub const TX_OVER: u32 = 1 << 3;
    pub const TX_EMPTY: u32 = 1 << 4;
    pub const RD_REQ: u32 = 1 << 5;
    pub const TX_ABRT: u32 = 1 << 6;
    pub const RX_DONE: u32 = 1 << 7;
    pub const ACTIVITY: u32 = 1 << 8;
    pub const STOP_DET: u32 = 1 << 9;
    pub const START_DET: u32 = 1 << 10;
    pub const GEN_CALL: u32 = 1 << 11;

    /// IC_CLR_INTR が落とすビット群（TX_ABRT/TX_EMPTY/RX_FULL は専用
    /// クリアかレベル条件なので含まれない）。
    pub const CLR_INTR_MASK: u32 = RX_UNDER
        | RX_OVER
        | TX_OVER
        | RD_REQ
        | RX_DONE
        | ACTIVITY
        | STOP_DET
        | START_DET
        | GEN_CALL;
}

/// IC_TX_ABRT_SOURCE の主要ビット。
mod abrt {
    pub const ABRT_7B_ADDR_NOACK: u32 = 1 << 0;
    pub const ABRT_TXDATA_NOACK: u32 = 1 << 3;
    pub const ABRT_SBYTE_NORSTRT: u32 = 1 << 9;
    pub const ABRT_MASTER_DIS: u32 = 1 << 11;
    pub const ARB_LOST: u32 = 1 << 12;
}

/// IC_STATUS ビット。
mod status {
    pub const ACTIVITY: u32 = 1 << 0;
    pub const TFNF: u32 = 1 << 1;
    pub const TFE: u32 = 1 << 2;
    pub const RFNE: u32 = 1 << 3;
    pub const RFF: u32 = 1 << 4;
    pub const MST_ACTIVITY: u32 = 1 << 5;
}

/// IC_ENABLE ビット。
mod enable {
    pub const ENABLE: u32 = 1 << 0;
}

/// IC_COMP_TYPE の固定値（"DW01" のマジックナンバー）。
const COMP_TYPE_VALUE: u32 = 0x4457_0140;
/// IC_COMP_VERSION。databook の ASCII 風バージョン符号化。
const COMP_VERSION_VALUE: u32 = 0x3230_312A;

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
pub struct DwApbI2cParams {
    /// combined 割り込み線。
    pub combined_irq: SignalId,
    /// RX FIFO 深さ（IC_RX_BUFFER_DEPTH 構成）。
    pub rx_depth: usize,
    /// コマンドキュー深さ（IC_TX_BUFFER_DEPTH 構成）。
    pub tx_depth: usize,
    /// 未マップオフセットの扱い。
    pub unmapped: UnmappedPolicy,
}

impl DwApbI2cParams {
    pub const fn new(combined_irq: SignalId) -> Self {
        Self {
            combined_irq,
            rx_depth: 16,
            tx_depth: 16,
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// スナップショット対象のスカラーレジスタ群（設計書 §10 の state 分離）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DwApbI2cState {
    pub schema: u32,
    pub con: u32,
    pub tar: u32,
    pub sar: u32,
    pub hs_maddr: u32,
    pub ss_hcnt: u32,
    pub ss_lcnt: u32,
    pub fs_hcnt: u32,
    pub fs_lcnt: u32,
    pub hs_hcnt: u32,
    pub hs_lcnt: u32,
    pub intr_mask: u32,
    pub rx_tl: u32,
    pub tx_tl: u32,
    pub enable: u32,
    pub sda_hold: u32,
    pub slv_data_nack_only: u32,
    pub dma_cr: u32,
    pub dma_tdlr: u32,
    pub dma_rdlr: u32,
    pub sda_setup: u32,
    pub ack_general_call: u32,
    pub raw_intr: u32,
    pub tx_abrt_source: u32,
    pub irq_level: bool,
}

impl Default for DwApbI2cState {
    fn default() -> Self {
        Self {
            schema: 1,
            // databook の POR: マスタ + fast + restart 有効。
            con: 0x65,
            tar: 0x55,
            sar: 0x55,
            hs_maddr: 0x01,
            // HCNT/LCNT は構成依存。機能モデルではクロックを生成しないので 0。
            ss_hcnt: 0,
            ss_lcnt: 0,
            fs_hcnt: 0,
            fs_lcnt: 0,
            hs_hcnt: 0,
            hs_lcnt: 0,
            intr_mask: 0,
            rx_tl: 0,
            tx_tl: 0,
            enable: 0,
            sda_hold: 0x0001,
            slv_data_nack_only: 0,
            dma_cr: 0,
            dma_tdlr: 0,
            dma_rdlr: 0,
            sda_setup: 0,
            ack_general_call: 0,
            raw_intr: 0,
            tx_abrt_source: 0,
            irq_level: false,
        }
    }
}

/// STOP までキューに積まれる 1 コマンド。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingCmd {
    data: u8,
    read: bool,
    stop: bool,
    restart: bool,
}

/// DW_apb_i2c マスタコントローラ。Initiator ポートは構築時に注入する
/// （ADR 0008: モデルはバスの実体を知らない）。
pub struct DwApbI2c {
    params: DwApbI2cParams,
    state: DwApbI2cState,
    rx: VecDeque<u8>,
    pending: Vec<PendingCmd>,
    initiator: Box<dyn I2cInitiator>,
}

impl DwApbI2c {
    pub fn new(params: DwApbI2cParams, initiator: Box<dyn I2cInitiator>) -> Self {
        let mut device = Self {
            params,
            state: DwApbI2cState::default(),
            rx: VecDeque::new(),
            pending: Vec::new(),
            initiator,
        };
        device.apply_speed();
        device
    }

    fn enabled(&self) -> bool {
        self.state.enable & enable::ENABLE != 0
    }

    fn apply_speed(&mut self) {
        let speed = match self.state.con & con::SPEED_MASK {
            con::SPEED_STANDARD => I2cSpeed::Standard,
            con::SPEED_FAST => I2cSpeed::Fast,
            con::SPEED_HIGH => I2cSpeed::HighSpeed,
            _ => I2cSpeed::Standard,
        };
        self.initiator.set_speed(speed);
    }

    fn status(&self) -> u32 {
        let mut value = status::TFNF | status::TFE;
        if !self.pending.is_empty() {
            value |= status::ACTIVITY | status::MST_ACTIVITY;
        }
        if !self.rx.is_empty() {
            value |= status::RFNE;
        }
        if self.rx.len() >= self.params.rx_depth {
            value |= status::RFF;
        }
        value
    }

    /// レベル駆動ビット（RX_FULL / TX_EMPTY）を FIFO 状態から再計算する。
    fn refresh_level_bits(&mut self) {
        if (self.rx.len() as u32) > self.state.rx_tl {
            self.state.raw_intr |= intr::RX_FULL;
        } else {
            self.state.raw_intr &= !intr::RX_FULL;
        }
        if self.enabled() {
            // コマンドは STOP 時に同期実行され TX FIFO は常に空。
            self.state.raw_intr |= intr::TX_EMPTY;
        } else {
            self.state.raw_intr &= !intr::TX_EMPTY;
        }
    }

    fn update_intr(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let asserted = self.state.raw_intr & self.state.intr_mask != 0;
        if asserted != self.state.irq_level {
            set_optional_signal(ctx, self.params.combined_irq, asserted)?;
            self.state.irq_level = asserted;
        }
        Ok(())
    }

    fn push_command(&mut self, ctx: &mut dyn Ctx, value: u32) {
        if !self.enabled() {
            // 無効化中の DATA_CMD 書き込みは破棄される。
            return;
        }
        if self.pending.len() >= self.params.tx_depth {
            self.state.raw_intr |= intr::TX_OVER;
            return;
        }
        let command = PendingCmd {
            data: (value & 0xFF) as u8,
            read: value & data_cmd::CMD_READ != 0,
            stop: value & data_cmd::STOP != 0,
            restart: value & data_cmd::RESTART != 0,
        };
        let stop = command.stop;
        self.pending.push(command);
        if stop {
            self.execute(ctx);
        }
    }

    /// STOP までに積まれたコマンドを 1 バッチとして実行する。
    fn execute(&mut self, ctx: &mut dyn Ctx) {
        let queue = core::mem::take(&mut self.pending);
        if queue.is_empty() {
            return;
        }
        // databook: マスタモード無効でのマスタ転送は ABRT_MASTER_DIS。
        if self.state.con & con::MASTER_MODE == 0 {
            self.abort(abrt::ABRT_MASTER_DIS);
            return;
        }
        let addr = match I2cAddr::seven((self.state.tar & 0x7F) as u8) {
            Ok(addr) => addr,
            Err(_) => {
                self.abort(abrt::ABRT_TXDATA_NOACK);
                return;
            }
        };
        let mut writes: Vec<u8> = Vec::new();
        let mut reads = 0usize;
        let mut read_restarts = false;
        let mut malformed = false;
        for command in &queue {
            if command.read {
                reads += 1;
                if reads == 1 {
                    read_restarts = command.restart;
                }
            } else if reads == 0 {
                writes.push(command.data);
            } else {
                // 読み出しフェーズの後の書き込みは modeled していない。
                malformed = true;
            }
        }
        if malformed {
            self.abort(abrt::ABRT_TXDATA_NOACK);
            return;
        }
        // databook: IC_RESTART_EN = 0 での RESTART は ABRT_SBYTE_NORSTRT。
        if read_restarts && self.state.con & con::RESTART_EN == 0 {
            self.abort(abrt::ABRT_SBYTE_NORSTRT);
            return;
        }
        let result = if reads == 0 {
            self.initiator.write(ctx, addr, &writes)
        } else if writes.is_empty() {
            let mut buf = alloc::vec![0u8; reads];
            let result = self.initiator.read(ctx, addr, &mut buf);
            if result.is_ok() {
                self.push_rx_bytes(&buf);
            }
            result
        } else if read_restarts {
            let mut buf = alloc::vec![0u8; reads];
            let result = self.initiator.write_read(ctx, addr, &writes, &mut buf);
            if result.is_ok() {
                self.push_rx_bytes(&buf);
            }
            result
        } else {
            let written = self.initiator.write(ctx, addr, &writes);
            if written.is_err() {
                written
            } else {
                let mut buf = alloc::vec![0u8; reads];
                let result = self.initiator.read(ctx, addr, &mut buf);
                if result.is_ok() {
                    self.push_rx_bytes(&buf);
                }
                result
            }
        };
        match result {
            Ok(()) => {
                self.state.raw_intr |= intr::STOP_DET | intr::START_DET | intr::ACTIVITY;
            }
            Err(error) => {
                let source = match error {
                    I2cError::Nack => abrt::ABRT_7B_ADDR_NOACK,
                    I2cError::ArbitrationLost => abrt::ARB_LOST,
                    _ => abrt::ABRT_TXDATA_NOACK,
                };
                self.abort(source);
            }
        }
    }

    fn abort(&mut self, source: u32) {
        self.state.tx_abrt_source = source;
        self.state.raw_intr |= intr::TX_ABRT;
        self.pending.clear();
        self.refresh_level_bits();
    }

    fn push_rx_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if self.rx.len() >= self.params.rx_depth {
                self.state.raw_intr |= intr::RX_OVER;
                continue;
            }
            self.rx.push_back(byte);
        }
        self.refresh_level_bits();
    }

    fn pop_rx(&mut self) -> u32 {
        match self.rx.pop_front() {
            Some(byte) => {
                self.refresh_level_bits();
                u32::from(byte)
            }
            None => {
                self.state.raw_intr |= intr::RX_UNDER;
                0
            }
        }
    }

    fn set_enable(&mut self, value: u32) {
        self.state.enable = value;
        if !self.enabled() {
            // databook: 無効化は TX/RX FIFO をフラッシュする。
            self.pending.clear();
            self.rx.clear();
        }
        self.refresh_level_bits();
    }

    fn clear(&mut self, offset: u64) -> u32 {
        let mask = match offset {
            reg::CLR_INTR => intr::CLR_INTR_MASK,
            reg::CLR_RX_UNDER => intr::RX_UNDER,
            reg::CLR_RX_OVER => intr::RX_OVER,
            reg::CLR_TX_OVER => intr::TX_OVER,
            reg::CLR_RD_REQ => intr::RD_REQ,
            reg::CLR_RX_DONE => intr::RX_DONE,
            reg::CLR_ACTIVITY => intr::ACTIVITY,
            reg::CLR_STOP_DET => intr::STOP_DET,
            reg::CLR_START_DET => intr::START_DET,
            reg::CLR_GEN_CALL => intr::GEN_CALL,
            reg::CLR_TX_ABRT => {
                self.state.tx_abrt_source = 0;
                intr::TX_ABRT
            }
            _ => 0,
        };
        self.state.raw_intr &= !mask;
        0
    }

    fn read_word(&mut self, offset: u64) -> Result<u32, BusFault> {
        let value = match offset {
            reg::CON => self.state.con,
            reg::TAR => self.state.tar,
            reg::SAR => self.state.sar,
            reg::HS_MADDR => self.state.hs_maddr,
            reg::DATA_CMD => self.pop_rx(),
            reg::SS_SCL_HCNT => self.state.ss_hcnt,
            reg::SS_SCL_LCNT => self.state.ss_lcnt,
            reg::FS_SCL_HCNT => self.state.fs_hcnt,
            reg::FS_SCL_LCNT => self.state.fs_lcnt,
            reg::HS_SCL_HCNT => self.state.hs_hcnt,
            reg::HS_SCL_LCNT => self.state.hs_lcnt,
            reg::INTR_STAT => self.state.raw_intr & self.state.intr_mask,
            reg::INTR_MASK => self.state.intr_mask,
            reg::RAW_INTR_STAT => self.state.raw_intr,
            reg::RX_TL => self.state.rx_tl,
            reg::TX_TL => self.state.tx_tl,
            reg::CLR_INTR
            | reg::CLR_RX_UNDER
            | reg::CLR_RX_OVER
            | reg::CLR_TX_OVER
            | reg::CLR_RD_REQ
            | reg::CLR_TX_ABRT
            | reg::CLR_RX_DONE
            | reg::CLR_ACTIVITY
            | reg::CLR_STOP_DET
            | reg::CLR_START_DET
            | reg::CLR_GEN_CALL => self.clear(offset),
            reg::ENABLE => self.state.enable,
            reg::STATUS => self.status(),
            reg::TXFLR => 0,
            reg::RXFLR => self.rx.len() as u32,
            reg::SDA_HOLD => self.state.sda_hold,
            reg::TX_ABRT_SOURCE => {
                // databook: 読み出しは TX_ABRT_SOURCE をクリアする。
                let source = self.state.tx_abrt_source;
                self.state.tx_abrt_source = 0;
                source
            }
            reg::SLV_DATA_NACK_ONLY => self.state.slv_data_nack_only,
            reg::DMA_CR => self.state.dma_cr,
            reg::DMA_TDLR => self.state.dma_tdlr,
            reg::DMA_RDLR => self.state.dma_rdlr,
            reg::SDA_SETUP => self.state.sda_setup,
            reg::ACK_GENERAL_CALL => self.state.ack_general_call,
            reg::ENABLE_STATUS => self.state.enable & enable::ENABLE,
            reg::COMP_TYPE => COMP_TYPE_VALUE,
            reg::COMP_VERSION => COMP_VERSION_VALUE,
            _ => return self.unmapped(offset),
        };
        Ok(value)
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u32) -> Result<(), BusFault> {
        match offset {
            reg::CON => {
                self.state.con = value & 0x7FF;
                self.apply_speed();
            }
            reg::TAR => self.state.tar = value & 0xFFFF,
            reg::SAR => self.state.sar = value & 0x3FF,
            reg::HS_MADDR => self.state.hs_maddr = value & 0x7,
            reg::DATA_CMD => self.push_command(ctx, value),
            reg::SS_SCL_HCNT => self.state.ss_hcnt = value & 0xFFFF,
            reg::SS_SCL_LCNT => self.state.ss_lcnt = value & 0xFFFF,
            reg::FS_SCL_HCNT => self.state.fs_hcnt = value & 0xFFFF,
            reg::FS_SCL_LCNT => self.state.fs_lcnt = value & 0xFFFF,
            reg::HS_SCL_HCNT => self.state.hs_hcnt = value & 0xFFFF,
            reg::HS_SCL_LCNT => self.state.hs_lcnt = value & 0xFFFF,
            reg::INTR_MASK => self.state.intr_mask = value & 0xFFF,
            reg::RX_TL => self.state.rx_tl = value & 0xFF,
            reg::TX_TL => self.state.tx_tl = value & 0xFF,
            reg::ENABLE => self.set_enable(value & 0x7),
            reg::SDA_HOLD => self.state.sda_hold = value & 0xFFFF,
            reg::SLV_DATA_NACK_ONLY => self.state.slv_data_nack_only = value & 0x1,
            reg::DMA_CR => self.state.dma_cr = value & 0x3,
            reg::DMA_TDLR => self.state.dma_tdlr = value & 0xFF,
            reg::DMA_RDLR => self.state.dma_rdlr = value & 0xFF,
            reg::SDA_SETUP => self.state.sda_setup = value & 0xFF,
            reg::ACK_GENERAL_CALL => self.state.ack_general_call = value & 0x1,
            // 読み出し専用レジスタと read-to-clear レジスタへの書き込みは無視。
            reg::INTR_STAT
            | reg::RAW_INTR_STAT
            | reg::STATUS
            | reg::TXFLR
            | reg::RXFLR
            | reg::TX_ABRT_SOURCE
            | reg::ENABLE_STATUS
            | reg::COMP_TYPE
            | reg::COMP_VERSION
            | reg::CLR_INTR
            | reg::CLR_RX_UNDER
            | reg::CLR_RX_OVER
            | reg::CLR_TX_OVER
            | reg::CLR_RD_REQ
            | reg::CLR_TX_ABRT
            | reg::CLR_RX_DONE
            | reg::CLR_ACTIVITY
            | reg::CLR_STOP_DET
            | reg::CLR_START_DET
            | reg::CLR_GEN_CALL => {}
            _ => return self.unmapped(offset).map(|_| ()),
        }
        self.update_intr(ctx).map_err(|_| BusFault::Device)?;
        Ok(())
    }

    fn unmapped(&self, _offset: u64) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for DwApbI2c {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            return Err(BusFault::InvalidSize);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(offset)?;
        // read-to-clear レジスタ（CLR_* / TX_ABRT_SOURCE）は読み出しが
        // 割り込みレベルを変えるので、ここでも線を更新する。
        self.update_intr(ctx).map_err(|_| BusFault::Device)?;
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
            // レジスタは 32 ビット幅。8 バイト転送は収まらない。
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for DwApbI2c {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

/// One command waiting in the TX FIFO. Host initiator identity is not saved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DwApbI2cCommand {
    pub data: u8,
    pub read: bool,
    pub stop: bool,
    pub restart: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DwApbI2cCheckpoint {
    pub schema: u32,
    pub registers: DwApbI2cState,
    pub rx: Vec<u8>,
    pub pending: Vec<DwApbI2cCommand>,
}

impl Snapshot for DwApbI2c {
    type State = DwApbI2cCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        DwApbI2cCheckpoint {
            schema: Self::SCHEMA_VERSION,
            registers: self.state,
            rx: self.rx.iter().copied().collect(),
            pending: self
                .pending
                .iter()
                .map(|command| DwApbI2cCommand {
                    data: command.data,
                    read: command.read,
                    stop: command.stop,
                    restart: command.restart,
                })
                .collect(),
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION
            || state.schema != Self::SCHEMA_VERSION
            || state.registers.schema != 1
        {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.rx.len() > self.params.rx_depth || state.pending.len() > self.params.tx_depth {
            return Err(SnapshotError::InvalidState);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.registers;
        self.rx = state.rx.iter().copied().collect();
        self.pending = state
            .pending
            .iter()
            .map(|command| PendingCmd {
                data: command.data,
                read: command.read,
                stop: command.stop,
                restart: command.restart,
            })
            .collect();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        let registers = &state.registers;
        for word in [
            registers.schema,
            registers.con,
            registers.tar,
            registers.sar,
            registers.hs_maddr,
            registers.ss_hcnt,
            registers.ss_lcnt,
            registers.fs_hcnt,
            registers.fs_lcnt,
            registers.hs_hcnt,
            registers.hs_lcnt,
            registers.intr_mask,
            registers.rx_tl,
            registers.tx_tl,
            registers.enable,
            registers.sda_hold,
            registers.slv_data_nack_only,
            registers.dma_cr,
            registers.dma_tdlr,
            registers.dma_rdlr,
            registers.sda_setup,
            registers.ack_general_call,
            registers.raw_intr,
            registers.tx_abrt_source,
        ] {
            hash.u32(word);
        }
        hash.bool(registers.irq_level);
        hash.bytes(&state.rx);
        hash.usize(state.pending.len());
        for command in &state.pending {
            hash.byte(command.data);
            hash.bool(command.read);
            hash.bool(command.stop);
            hash.bool(command.restart);
        }
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
    use mosaic_core_types::{AccessKind, Endian, EventId, NodeId, Security, SnapshotError, VTime};

    const IRQ: SignalId = SignalId(7);

    #[derive(Debug, PartialEq, Eq)]
    enum Op {
        Write(I2cAddr, Vec<u8>),
        Read(I2cAddr, usize),
        WriteRead(I2cAddr, Vec<u8>, usize),
    }

    #[derive(Default)]
    struct Fake {
        speeds: Vec<I2cSpeed>,
        ops: Vec<Op>,
        fail: Option<I2cError>,
        read_fill: u8,
    }

    struct FakeHandle(Arc<Mutex<Fake>>);

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    impl I2cInitiator for FakeHandle {
        fn set_speed(&mut self, speed: I2cSpeed) {
            lock(&self.0).speeds.push(speed);
        }
        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            addr: I2cAddr,
            bytes: &[u8],
        ) -> Result<(), I2cError> {
            let mut fake = lock(&self.0);
            fake.ops.push(Op::Write(addr, bytes.to_vec()));
            match fake.fail {
                None => Ok(()),
                Some(error) => Err(error),
            }
        }
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            addr: I2cAddr,
            buf: &mut [u8],
        ) -> Result<(), I2cError> {
            let mut fake = lock(&self.0);
            fake.ops.push(Op::Read(addr, buf.len()));
            if let Some(error) = fake.fail {
                return Err(error);
            }
            let mut fill = fake.read_fill;
            for slot in buf.iter_mut() {
                *slot = fill;
                fill = fill.wrapping_add(1);
            }
            fake.read_fill = fill;
            Ok(())
        }
        fn write_read(
            &mut self,
            _ctx: &mut dyn Ctx,
            addr: I2cAddr,
            out: &[u8],
            input: &mut [u8],
        ) -> Result<(), I2cError> {
            let mut fake = lock(&self.0);
            fake.ops
                .push(Op::WriteRead(addr, out.to_vec(), input.len()));
            if let Some(error) = fake.fail {
                return Err(error);
            }
            let mut fill = fake.read_fill;
            for slot in input.iter_mut() {
                *slot = fill;
                fill = fill.wrapping_add(1);
            }
            fake.read_fill = fill;
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

    fn device() -> (DwApbI2c, Arc<Mutex<Fake>>, TestCtx) {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let device = DwApbI2c::new(
            DwApbI2cParams::new(IRQ),
            Box::new(FakeHandle(Arc::clone(&fake))),
        );
        (
            device,
            fake,
            TestCtx {
                signals: Vec::new(),
            },
        )
    }

    fn read(device: &mut DwApbI2c, ctx: &mut TestCtx, offset: u64) -> u32 {
        device.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn write(device: &mut DwApbI2c, ctx: &mut TestCtx, offset: u64, value: u32) {
        device
            .write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn por_values_match_the_databook() {
        let (mut device, fake, mut ctx) = device();
        assert_eq!(read(&mut device, &mut ctx, reg::COMP_TYPE), 0x4457_0140);
        assert_eq!(read(&mut device, &mut ctx, reg::COMP_VERSION), 0x3230_312A);
        assert_eq!(read(&mut device, &mut ctx, reg::CON), 0x65);
        assert_eq!(read(&mut device, &mut ctx, reg::TAR), 0x55);
        assert_eq!(read(&mut device, &mut ctx, reg::ENABLE), 0);
        assert_eq!(read(&mut device, &mut ctx, reg::ENABLE_STATUS), 0);
        // TX FIFO 空 + RX FIFO 空。
        assert_eq!(read(&mut device, &mut ctx, reg::STATUS), 0x6);
        assert_eq!(read(&mut device, &mut ctx, reg::TXFLR), 0);
        assert_eq!(read(&mut device, &mut ctx, reg::RXFLR), 0);
        // POR の CON (fast) が構築時に initiator へ伝わる。
        assert_eq!(lock(&fake).speeds, vec![I2cSpeed::Fast]);
    }

    #[test]
    fn data_cmd_is_ignored_while_disabled() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::DATA_CMD, 0xD0 | data_cmd::STOP);
        assert_eq!(
            read(&mut device, &mut ctx, reg::STATUS) & status::ACTIVITY,
            0
        );
        assert!(lock(&fake).ops.is_empty());
    }

    #[test]
    fn stop_executes_the_queued_write_batch() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20);
        // STOP 前はキューに積まれるだけ。
        assert_eq!(lock(&fake).ops.len(), 0);
        assert_ne!(
            read(&mut device, &mut ctx, reg::STATUS) & status::ACTIVITY,
            0
        );
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x01 | data_cmd::STOP);
        assert_eq!(
            lock(&fake).ops,
            vec![Op::Write(I2cAddr::Seven(0x76), vec![0x20, 0x01])]
        );
        let raw = read(&mut device, &mut ctx, reg::RAW_INTR_STAT);
        assert_ne!(raw & intr::STOP_DET, 0);
        assert_ne!(raw & intr::START_DET, 0);
        assert_ne!(raw & intr::ACTIVITY, 0);
        // read-to-clear レジスタで落ちる。
        read(&mut device, &mut ctx, reg::CLR_STOP_DET);
        assert_eq!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::STOP_DET,
            0
        );
    }

    #[test]
    fn restart_read_becomes_one_write_read_transaction() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0xD0);
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::RESTART | data_cmd::STOP,
        );
        assert_eq!(
            lock(&fake).ops,
            vec![Op::WriteRead(I2cAddr::Seven(0x76), vec![0xD0], 1)]
        );
        // 実行は即時: RFNE が立って DATA_CMD から値が取り出せる。
        assert_ne!(read(&mut device, &mut ctx, reg::STATUS) & status::RFNE, 0);
        assert_eq!(read(&mut device, &mut ctx, reg::RXFLR), 1);
        assert_eq!(read(&mut device, &mut ctx, reg::DATA_CMD), 0x00);
        assert_eq!(read(&mut device, &mut ctx, reg::RXFLR), 0);
    }

    #[test]
    fn multi_byte_restart_read_fills_the_rx_fifo_in_order() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0xF7);
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::RESTART,
        );
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::STOP,
        );
        assert_eq!(
            lock(&fake).ops,
            vec![Op::WriteRead(I2cAddr::Seven(0x76), vec![0xF7], 2)]
        );
        assert_eq!(read(&mut device, &mut ctx, reg::RXFLR), 2);
        assert_eq!(read(&mut device, &mut ctx, reg::DATA_CMD), 0x00);
        assert_eq!(read(&mut device, &mut ctx, reg::DATA_CMD), 0x01);
        assert_eq!(read(&mut device, &mut ctx, reg::STATUS) & status::RFNE, 0);
    }

    #[test]
    fn write_then_read_without_restart_are_two_transactions() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20);
        write(&mut device, &mut ctx, reg::DATA_CMD, data_cmd::CMD_READ);
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::STOP,
        );
        assert_eq!(
            lock(&fake).ops,
            vec![
                Op::Write(I2cAddr::Seven(0x76), vec![0x20]),
                Op::Read(I2cAddr::Seven(0x76), 2),
            ]
        );
    }

    #[test]
    fn rx_under_is_reported_and_cleared() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        assert_eq!(read(&mut device, &mut ctx, reg::DATA_CMD), 0);
        assert_ne!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::RX_UNDER,
            0
        );
        read(&mut device, &mut ctx, reg::CLR_RX_UNDER);
        assert_eq!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::RX_UNDER,
            0
        );
    }

    #[test]
    fn nack_raises_tx_abrt_with_source_and_clears_it() {
        let (mut device, fake, mut ctx) = device();
        lock(&fake).fail = Some(I2cError::Nack);
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x41);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20 | data_cmd::STOP);
        assert_ne!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::TX_ABRT,
            0
        );
        // 読み出しで source が返り、同時にクリアされる（databook 準拠）。
        assert_eq!(read(&mut device, &mut ctx, reg::TX_ABRT_SOURCE), 0x01);
        assert_eq!(read(&mut device, &mut ctx, reg::TX_ABRT_SOURCE), 0x00);
        read(&mut device, &mut ctx, reg::CLR_TX_ABRT);
        assert_eq!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::TX_ABRT,
            0
        );
        assert_eq!(
            read(&mut device, &mut ctx, reg::STATUS) & status::ACTIVITY,
            0
        );
    }

    #[test]
    fn reserved_addresses_abort_without_touching_the_bus() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x00);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20 | data_cmd::STOP);
        assert!(lock(&fake).ops.is_empty());
        assert_eq!(read(&mut device, &mut ctx, reg::TX_ABRT_SOURCE), 0x08);
    }

    #[test]
    fn master_mode_disabled_aborts_with_master_dis() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        // MASTER_MODE ビットを落とした CON へ書き換える。
        write(
            &mut device,
            &mut ctx,
            reg::CON,
            con::SPEED_FAST | con::RESTART_EN,
        );
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20 | data_cmd::STOP);
        assert!(lock(&fake).ops.is_empty());
        assert_eq!(read(&mut device, &mut ctx, reg::TX_ABRT_SOURCE), 1 << 11);
    }

    #[test]
    fn restart_without_restart_en_aborts() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        // POR の CON から RESTART_EN だけ落とす。
        write(
            &mut device,
            &mut ctx,
            reg::CON,
            con::MASTER_MODE | con::SPEED_FAST,
        );
        write(&mut device, &mut ctx, reg::DATA_CMD, 0xD0);
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::RESTART | data_cmd::STOP,
        );
        assert!(lock(&fake).ops.is_empty());
        assert_eq!(read(&mut device, &mut ctx, reg::TX_ABRT_SOURCE), 1 << 9);
    }

    #[test]
    fn irq_line_follows_masked_raw_status() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        // TX_EMPTY は有効化で立つが、マスク 0 なので IRQ は出ない。
        assert_ne!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::TX_EMPTY,
            0
        );
        assert!(ctx.signals.is_empty());
        write(&mut device, &mut ctx, reg::INTR_MASK, intr::STOP_DET);
        assert_eq!(read(&mut device, &mut ctx, reg::INTR_STAT), 0);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20 | data_cmd::STOP);
        assert_eq!(read(&mut device, &mut ctx, reg::INTR_STAT), intr::STOP_DET);
        assert_eq!(ctx.signals, vec![(IRQ, true)]);
        read(&mut device, &mut ctx, reg::CLR_STOP_DET);
        // read-to-clear が即座に割り込み線を落とす（実機の MIS 追従と同じ）。
        assert_eq!(ctx.signals, vec![(IRQ, true), (IRQ, false)]);
    }

    #[test]
    fn disable_flushes_queue_and_rx_fifo() {
        let (mut device, fake, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(&mut device, &mut ctx, reg::DATA_CMD, 0x20);
        write(&mut device, &mut ctx, reg::ENABLE, 0);
        assert!(lock(&fake).ops.is_empty());
        assert_eq!(
            read(&mut device, &mut ctx, reg::STATUS) & status::ACTIVITY,
            0
        );
        assert_eq!(read(&mut device, &mut ctx, reg::RXFLR), 0);
        assert_eq!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::TX_EMPTY,
            0
        );
    }

    #[test]
    fn rx_full_is_a_level_bit_against_rx_tl() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::RX_TL, 0);
        write(&mut device, &mut ctx, reg::TAR, 0x76);
        write(
            &mut device,
            &mut ctx,
            reg::DATA_CMD,
            data_cmd::CMD_READ | data_cmd::STOP,
        );
        assert_ne!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::RX_FULL,
            0
        );
        read(&mut device, &mut ctx, reg::DATA_CMD);
        assert_eq!(
            read(&mut device, &mut ctx, reg::RAW_INTR_STAT) & intr::RX_FULL,
            0
        );
    }

    #[test]
    fn speed_field_reaches_the_initiator() {
        let (mut device, fake, mut ctx) = device();
        write(
            &mut device,
            &mut ctx,
            reg::CON,
            con::MASTER_MODE | con::SPEED_HIGH | con::RESTART_EN,
        );
        assert_eq!(
            lock(&fake).speeds,
            vec![I2cSpeed::Fast, I2cSpeed::HighSpeed]
        );
    }

    #[test]
    fn read_only_registers_ignore_writes() {
        let (mut device, _, mut ctx) = device();
        write(&mut device, &mut ctx, reg::ENABLE, 1);
        write(&mut device, &mut ctx, reg::STATUS, 0xFFFF_FFFF);
        assert_eq!(read(&mut device, &mut ctx, reg::STATUS), 0x6);
        write(&mut device, &mut ctx, reg::COMP_TYPE, 0);
        assert_eq!(read(&mut device, &mut ctx, reg::COMP_TYPE), 0x4457_0140);
    }

    #[test]
    fn unmapped_and_oversized_accesses_follow_policy() {
        let (mut device, _, mut ctx) = device();
        // 0xA0..0xF4 は予約領域: RazWi。
        assert_eq!(read(&mut device, &mut ctx, 0x0A0), 0);
        write(&mut device, &mut ctx, 0x0A0, 1);
        assert_eq!(
            device.write(&mut ctx, reg::DATA_CMD, AccessSize::DoubleWord, 0, &attr()),
            Err(BusFault::InvalidSize)
        );
        let mut strict = DwApbI2cParams::new(IRQ);
        strict.unmapped = UnmappedPolicy::Fault;
        let mut strict_device = DwApbI2c::new(
            strict,
            Box::new(FakeHandle(Arc::new(Mutex::new(Fake::default())))),
        );
        assert_eq!(
            strict_device.read(&mut ctx, 0x0A0, AccessSize::Word, &attr()),
            Err(BusFault::Device)
        );
    }

    #[test]
    fn command_queue_and_rx_restore_without_replacing_the_initiator() {
        use mosaic_core_types::Snapshot;
        let (mut device, _, _) = device();
        device.state.tar = 0x55;
        device.rx.push_back(0x11);
        device.pending.push(PendingCmd {
            data: 0x22,
            read: true,
            stop: true,
            restart: false,
        });
        let saved = device.save();
        device.state.tar = 0;
        device.rx.clear();
        device.pending.clear();
        device.load(DwApbI2c::SCHEMA_VERSION, &saved).unwrap();
        assert_eq!(device.state.tar, 0x55);
        assert_eq!(device.rx.pop_front(), Some(0x11));
        assert_eq!(
            device.pending.first().copied(),
            Some(PendingCmd {
                data: 0x22,
                read: true,
                stop: true,
                restart: false,
            })
        );
        let mut too_deep = saved;
        too_deep.rx = vec![0; device.params.rx_depth + 1];
        assert_eq!(
            device.load(DwApbI2c::SCHEMA_VERSION, &too_deep),
            Err(SnapshotError::InvalidState)
        );
    }
}
