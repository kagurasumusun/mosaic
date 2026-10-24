//! STM32F1 リセット・クロック制御 (RCC) の機能レジスタモデル (RM0008 §7-§9)。
//!
//! 0x4002_1000 の RCC レジスタブロック (CR/CFGR/CIR/APB2RSTR/APB1RSTR/AHBENR/
//! APB2ENR/APB1ENR/BDCR/CSR) を、クロックツリー計算とゲート出力付きで表す。
//!
//! - **クロックツリー**: HSI (8 MHz 固定) / HSE (パラメータ) / PLL (HSI/2 または
//!   HSE、×2..×16) から SYSCLK を計算し、HPRE/PPRE1/PPRE2 で HCLK/PCLK1/PCLK2 を
//!   導出する。周波数は [`RccF1::sysclk_hz`] 等のアクセサで観測できる
//!   (STM32 には周波数読み出しレジスタが無いため、これはテスト・構成検証用)。
//! - **RDY ビット**: ロック時間を仮想時間でモデル化せず、ON ビットに追従して
//!   同期的に立つ (決定論優先; 実機の HSE 起動は数百 µs オーダ)。
//! - **ゲート出力**: AHBENR/APB1ENR/APB2ENR の各イネーブルビットは
//!   [`GATE_*`](self) 定数のシグナル出力になる。ビットが変化したときだけ
//!   `ctx.set_signal` が発行され、構成ルートは使うゲートだけを
//!   `Machine::connect` で配線すればよい (配線されていない出力は
//!   `set_signal` の UnknownSignal として無視される — PL190 と同じ挙動)。
//! - **CIR**: レディフラグ/イネーブル/クリアビット。レディ要因と CIR の
//!   イネーブルが重なると割り込み出力 (`RccF1Params::irq`) が立つ。
//!
//! 境界 (設計上の未実装):
//! - CSS (クロックセキュリティ) はフラグの保持のみ。
//! - APB2RSTR/APB1RSTR のリセットパルスは兄弟デバイスへ伝播しない
//!   (クロックゲートと同じ配線方式での伝播は後続作業)。
//! - BDCR/CSR の LSE/LSI はレジスタの保持のみで RTC への供給は無い。
//! - 72 MHz 上限は検証しない (ゲストの責任)。
//! - 予約領域は raz-wi。

#![forbid(unsafe_code)]

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{AccessSize, Attr, BusFault, SignalId};

// クロック・イネーブルビット ↔ ゲート出力シグナル ID の対応表。
// (シグナル ID は RCC ノードのシグナル空間。AHB=0x00.., APB1=0x10.., APB2=0x30..)
//
// レジスタ | ビット | 定数 |
// --- | --- | --- |
// AHBENR | 0 DMA1 | [`GATE_AHB_DMA1`] |
// AHBENR | 2 SRAM | [`GATE_AHB_SRAM`] |
// AHBENR | 4 FLITF | [`GATE_AHB_FLITF`] |
// AHBENR | 6 CRC | [`GATE_AHB_CRC`] |
// APB2ENR | 0 AFIO | [`GATE_APB2_AFIO`] |
// APB2ENR | 2 IOPA | [`GATE_APB2_IOPA`] |
// APB2ENR | 3 IOPB | [`GATE_APB2_IOPB`] |
// APB2ENR | 4 IOPC | [`GATE_APB2_IOPC`] |
// APB2ENR | 5 IOPD | [`GATE_APB2_IOPD`] |
// APB2ENR | 6 IOPE | [`GATE_APB2_IOPE`] |
// APB2ENR | 9 ADC1 | [`GATE_APB2_ADC1`] |
// APB2ENR | 10 ADC2 | [`GATE_APB2_ADC2`] |
// APB2ENR | 11 TIM1 | [`GATE_APB2_TIM1`] |
// APB2ENR | 12 SPI1 | [`GATE_APB2_SPI1`] |
// APB2ENR | 14 USART1 | [`GATE_APB2_USART1`] |
// APB1ENR | 0 TIM2 | [`GATE_APB1_TIM2`] |
// APB1ENR | 1 TIM3 | [`GATE_APB1_TIM3`] |
// APB1ENR | 2 TIM4 | [`GATE_APB1_TIM4`] |
// APB1ENR | 11 WWDG | [`GATE_APB1_WWDG`] |
// APB1ENR | 14 SPI2 | [`GATE_APB1_SPI2`] |
// APB1ENR | 17 USART2 | [`GATE_APB1_USART2`] |
// APB1ENR | 18 USART3 | [`GATE_APB1_USART3`] |
// APB1ENR | 21 I2C1 | [`GATE_APB1_I2C1`] |
// APB1ENR | 22 I2C2 | [`GATE_APB1_I2C2`] |
// APB1ENR | 23 USB | [`GATE_APB1_USB`] |
// APB1ENR | 25 CAN1 | [`GATE_APB1_CAN1`] |
// APB1ENR | 27 BKP | [`GATE_APB1_BKP`] |
// APB1ENR | 28 PWR | [`GATE_APB1_PWR`] |

/// AHBENR ゲートのシグナル ID 基点。
const GATE_AHB_BASE: u32 = 0x00;
/// APB1ENR ゲートのシグナル ID 基点。
const GATE_APB1_BASE: u32 = 0x10;
/// APB2ENR ゲートのシグナル ID 基点。
const GATE_APB2_BASE: u32 = 0x30;

pub const GATE_AHB_DMA1: SignalId = SignalId(GATE_AHB_BASE);
pub const GATE_AHB_DMA2: SignalId = SignalId(GATE_AHB_BASE + 1);
pub const GATE_AHB_SRAM: SignalId = SignalId(GATE_AHB_BASE + 2);
pub const GATE_AHB_FLITF: SignalId = SignalId(GATE_AHB_BASE + 4);
pub const GATE_AHB_CRC: SignalId = SignalId(GATE_AHB_BASE + 6);

pub const GATE_APB1_TIM2: SignalId = SignalId(GATE_APB1_BASE);
pub const GATE_APB1_TIM3: SignalId = SignalId(GATE_APB1_BASE + 1);
pub const GATE_APB1_TIM4: SignalId = SignalId(GATE_APB1_BASE + 2);
pub const GATE_APB1_TIM5: SignalId = SignalId(GATE_APB1_BASE + 3);
pub const GATE_APB1_TIM6: SignalId = SignalId(GATE_APB1_BASE + 4);
pub const GATE_APB1_TIM7: SignalId = SignalId(GATE_APB1_BASE + 5);
pub const GATE_APB1_TIM12: SignalId = SignalId(GATE_APB1_BASE + 6);
pub const GATE_APB1_TIM13: SignalId = SignalId(GATE_APB1_BASE + 7);
pub const GATE_APB1_TIM14: SignalId = SignalId(GATE_APB1_BASE + 8);
pub const GATE_APB1_WWDG: SignalId = SignalId(GATE_APB1_BASE + 11);
pub const GATE_APB1_SPI2: SignalId = SignalId(GATE_APB1_BASE + 14);
pub const GATE_APB1_SPI3: SignalId = SignalId(GATE_APB1_BASE + 15);
pub const GATE_APB1_USART2: SignalId = SignalId(GATE_APB1_BASE + 17);
pub const GATE_APB1_USART3: SignalId = SignalId(GATE_APB1_BASE + 18);
pub const GATE_APB1_UART4: SignalId = SignalId(GATE_APB1_BASE + 19);
pub const GATE_APB1_UART5: SignalId = SignalId(GATE_APB1_BASE + 20);
pub const GATE_APB1_I2C1: SignalId = SignalId(GATE_APB1_BASE + 21);
pub const GATE_APB1_I2C2: SignalId = SignalId(GATE_APB1_BASE + 22);
pub const GATE_APB1_USB: SignalId = SignalId(GATE_APB1_BASE + 23);
pub const GATE_APB1_CAN1: SignalId = SignalId(GATE_APB1_BASE + 25);
pub const GATE_APB1_BKP: SignalId = SignalId(GATE_APB1_BASE + 27);
pub const GATE_APB1_PWR: SignalId = SignalId(GATE_APB1_BASE + 28);
pub const GATE_APB1_DAC: SignalId = SignalId(GATE_APB1_BASE + 29);

pub const GATE_APB2_AFIO: SignalId = SignalId(GATE_APB2_BASE);
pub const GATE_APB2_IOPA: SignalId = SignalId(GATE_APB2_BASE + 2);
pub const GATE_APB2_IOPB: SignalId = SignalId(GATE_APB2_BASE + 3);
pub const GATE_APB2_IOPC: SignalId = SignalId(GATE_APB2_BASE + 4);
pub const GATE_APB2_IOPD: SignalId = SignalId(GATE_APB2_BASE + 5);
pub const GATE_APB2_IOPE: SignalId = SignalId(GATE_APB2_BASE + 6);
pub const GATE_APB2_IOPF: SignalId = SignalId(GATE_APB2_BASE + 7);
pub const GATE_APB2_IOPG: SignalId = SignalId(GATE_APB2_BASE + 8);
pub const GATE_APB2_ADC1: SignalId = SignalId(GATE_APB2_BASE + 9);
pub const GATE_APB2_ADC2: SignalId = SignalId(GATE_APB2_BASE + 10);
pub const GATE_APB2_TIM1: SignalId = SignalId(GATE_APB2_BASE + 11);
pub const GATE_APB2_SPI1: SignalId = SignalId(GATE_APB2_BASE + 12);
pub const GATE_APB2_TIM8: SignalId = SignalId(GATE_APB2_BASE + 13);
pub const GATE_APB2_USART1: SignalId = SignalId(GATE_APB2_BASE + 14);

/// (レジスタビット位置, シグナル ID) の対応表。ENR 書き込み時の差分配送に使う。
const AHBENR_GATES: [(u32, SignalId); 5] = [
    (0, GATE_AHB_DMA1),
    (1, GATE_AHB_DMA2),
    (2, GATE_AHB_SRAM),
    (4, GATE_AHB_FLITF),
    (6, GATE_AHB_CRC),
];

const APB1ENR_GATES: [(u32, SignalId); 23] = [
    (0, GATE_APB1_TIM2),
    (1, GATE_APB1_TIM3),
    (2, GATE_APB1_TIM4),
    (3, GATE_APB1_TIM5),
    (4, GATE_APB1_TIM6),
    (5, GATE_APB1_TIM7),
    (6, GATE_APB1_TIM12),
    (7, GATE_APB1_TIM13),
    (8, GATE_APB1_TIM14),
    (11, GATE_APB1_WWDG),
    (14, GATE_APB1_SPI2),
    (15, GATE_APB1_SPI3),
    (17, GATE_APB1_USART2),
    (18, GATE_APB1_USART3),
    (19, GATE_APB1_UART4),
    (20, GATE_APB1_UART5),
    (21, GATE_APB1_I2C1),
    (22, GATE_APB1_I2C2),
    (23, GATE_APB1_USB),
    (25, GATE_APB1_CAN1),
    (27, GATE_APB1_BKP),
    (28, GATE_APB1_PWR),
    (29, GATE_APB1_DAC),
];

const APB2ENR_GATES: [(u32, SignalId); 15] = [
    (0, GATE_APB2_AFIO),
    (2, GATE_APB2_IOPA),
    (3, GATE_APB2_IOPB),
    (4, GATE_APB2_IOPC),
    (5, GATE_APB2_IOPD),
    (6, GATE_APB2_IOPE),
    (7, GATE_APB2_IOPF),
    (8, GATE_APB2_IOPG),
    (9, GATE_APB2_ADC1),
    (10, GATE_APB2_ADC2),
    (11, GATE_APB2_TIM1),
    (12, GATE_APB2_SPI1),
    (13, GATE_APB2_TIM8),
    (14, GATE_APB2_USART1),
    (15, SignalId(GATE_APB2_BASE + 15)), // ADC3EN (高密度)
];

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct RccF1Params {
    /// RCC グローバル割り込み出力 (NVIC 位置 5) のシグナル ID。
    pub irq: SignalId,
    /// 外部発振子の周波数。Blue Pill 等の典型的構成は 8 MHz。
    pub hse_hz: u32,
    /// 内部 8 MHz 発振器。データシート上は工場較正付きの固定周波数。
    pub hsi_hz: u32,
}

impl RccF1Params {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            hse_hz: 8_000_000,
            hsi_hz: 8_000_000,
        }
    }

    pub fn with_hse_hz(mut self, hse_hz: u32) -> Self {
        self.hse_hz = hse_hz;
        self
    }
}

/// RCC レジスタの生の保持値。CR の RDY ビットと CFGR の SWS は読み出し時に合成する。
#[derive(Clone, Copy, Debug)]
struct RccState {
    /// CR の書き込み可能ビット (HSION/HSITRIM/HSEON/HSEBYP/CSSON/PLLON)。
    cr_writable: u32,
    /// CFGR の保持値 (SW ビットは要求値、SWS は合成)。
    cfgr: u32,
    /// CIR の割り込みイネーブルビット [15:8]。
    cir_enable: u32,
    /// CIR のレディフラグ [7:0]。
    cir_flags: u32,
    /// 実際に選択されているシステムクロック (SWS の値 0/1/2)。
    sws: u32,
    apb2rstr: u32,
    apb1rstr: u32,
    ahbenr: u32,
    apb2enr: u32,
    apb1enr: u32,
    bdcr: u32,
    csr: u32,
    /// 割り込み出力の現在レベル。
    irq_level: bool,
}

impl Default for RccState {
    fn default() -> Self {
        Self {
            // リセット値: HSION=1, HSITRIM=0x10 (既定 16)。
            cr_writable: 0x0000_0081,
            cfgr: 0,
            cir_enable: 0,
            cir_flags: 0,
            // HSI で起動。
            sws: 0,
            apb2rstr: 0,
            apb1rstr: 0,
            ahbenr: 0,
            apb2enr: 0,
            apb1enr: 0,
            bdcr: 0,
            csr: 0,
            irq_level: false,
        }
    }
}

/// STM32F1 RCC の機能モデル。
pub struct RccF1 {
    params: RccF1Params,
    state: RccState,
    // Last levels successfully queued for gate outputs. Keeping these separate
    // from ENR registers lets a later access retry edges rejected by a full
    // runtime event queue.
    emitted_ahbenr: u32,
    emitted_apb1enr: u32,
    emitted_apb2enr: u32,
}

impl RccF1 {
    pub fn new(params: RccF1Params) -> Self {
        Self {
            params,
            state: RccState::default(),
            emitted_ahbenr: 0,
            emitted_apb1enr: 0,
            emitted_apb2enr: 0,
        }
    }

    /// CR の合成読み出し値: RDY ビットは ON ビットに追従する。
    fn cr_read(&self) -> u32 {
        let mut value = self.state.cr_writable;
        if self.hsi_on() {
            value |= 1 << 1; // HSIRDY
        }
        if self.hse_on() {
            value |= 1 << 17; // HSERDY
        }
        if self.pll_on() {
            value |= 1 << 25; // PLLRDY
        }
        value
    }

    fn hsi_on(&self) -> bool {
        self.state.cr_writable & (1 << 0) != 0
    }

    fn hse_on(&self) -> bool {
        self.state.cr_writable & (1 << 16) != 0
    }

    fn pll_on(&self) -> bool {
        self.state.cr_writable & (1 << 24) != 0
    }

    /// PLL 入力周波数 (PLLON に関わらず設定から計算)。
    fn pll_input_hz(&self) -> u32 {
        let cfgr = self.state.cfgr;
        if cfgr & (1 << 16) == 0 {
            // PLLSRC=0: HSI/2。
            self.params.hsi_hz / 2
        } else if cfgr & (1 << 17) == 0 {
            // PLLXTPRE=0: HSE をそのまま。
            self.params.hse_hz
        } else {
            self.params.hse_hz / 2
        }
    }

    /// PLL 逓倍率 (×2..×16; 1111 は ×16 にクランプ)。
    fn pll_multiplier(&self) -> u32 {
        let mul = (self.state.cfgr >> 18) & 0xF;
        if mul >= 0xE {
            16
        } else {
            mul + 2
        }
    }

    /// 現在選択されているシステムクロック周波数 (Hz)。
    pub fn sysclk_hz(&self) -> u32 {
        match self.state.sws {
            1 => {
                if self.hse_on() {
                    self.params.hse_hz
                } else {
                    0
                }
            }
            2 => {
                if self.pll_on() {
                    self.pll_input_hz().saturating_mul(self.pll_multiplier())
                } else {
                    0
                }
            }
            _ => {
                if self.hsi_on() {
                    self.params.hsi_hz
                } else {
                    0
                }
            }
        }
    }

    /// HPRE による分周比。
    fn ahb_div(&self) -> u32 {
        let hpre = (self.state.cfgr >> 4) & 0xF;
        match hpre {
            0x8..=0xB => 1 << (hpre - 0x7),
            0xC..=0xF => 1 << (hpre - 0x6),
            _ => 1,
        }
    }

    /// AHB バスクロック (HCLK) 周波数 (Hz)。
    pub fn hclk_hz(&self) -> u32 {
        self.sysclk_hz() / self.ahb_div().max(1)
    }

    /// PPRE による分周比 (フィールド値 0b1xx のみ分周)。
    fn apb_div(field: u32) -> u32 {
        match field {
            0b100 => 2,
            0b101 => 4,
            0b110 => 8,
            0b111 => 16,
            _ => 1,
        }
    }

    /// APB1 (PCLK1) 周波数 (Hz)。
    pub fn pclk1_hz(&self) -> u32 {
        self.hclk_hz() / Self::apb_div((self.state.cfgr >> 8) & 0b111)
    }

    /// APB2 (PCLK2) 周波数 (Hz)。
    pub fn pclk2_hz(&self) -> u32 {
        self.hclk_hz() / Self::apb_div((self.state.cfgr >> 11) & 0b111)
    }

    /// SW 要求が切り替え可能か (対象クロックが準備済みか) を判定し、
    /// 可能なら SWS を更新する。CIR のレディフラグ変化もここで処理する。
    fn update_sws(&mut self) {
        let requested = self.state.cfgr & 0b11;
        let ready = match requested {
            0 => self.hsi_on(),
            1 => self.hse_on(),
            2 => self.pll_on(),
            _ => false,
        };
        if ready {
            self.state.sws = requested;
        } else if self.state.sws == requested {
            // RM0008: 選択中のクロックが停止するとハードウェアは HSI へ戻す。
            self.state.sws = 0;
        }
    }

    /// CIR フラグと割り込み出力を更新する。レディフラグは立ち上がり
    /// エッジでラッチされ、C ビットへの 1 書き込みでのみクリアされる
    /// (RM0008 §7.3.4)。クロック停止では再セットもクリアも起きない。
    fn latch_ready_edges(&mut self, old_cr: u32, old_bdcr: u32, old_csr: u32) {
        let rising = |old: bool, new: bool| old != new && new;
        if rising(old_cr & (1 << 0) != 0, self.hsi_on()) {
            self.state.cir_flags |= 1 << 2; // HSIRDYF
        }
        if rising(old_cr & (1 << 16) != 0, self.hse_on()) {
            self.state.cir_flags |= 1 << 3; // HSERDYF
        }
        if rising(old_cr & (1 << 24) != 0, self.pll_on()) {
            self.state.cir_flags |= 1 << 4; // PLLRDYF
        }
        if rising(old_csr & 1 != 0, self.state.csr & 1 != 0) {
            self.state.cir_flags |= 1 << 0; // LSIRDYF
        }
        if rising(old_bdcr & 1 != 0, self.state.bdcr & 1 != 0) {
            self.state.cir_flags |= 1 << 1; // LSERDYF
        }
    }

    /// Queue every output transition whose last delivery did not succeed.
    /// Continue after individual failures so one full-queue edge cannot starve
    /// unrelated IRQ or clock-gate outputs.
    fn deliver_gates(
        ctx: &mut dyn Ctx,
        table: &[(u32, SignalId)],
        desired: u32,
        emitted: &mut u32,
    ) -> Option<ContextError> {
        let mut first_error = None;
        for (bit, signal) in table {
            let mask = 1u32 << bit;
            let high = desired & mask != 0;
            let was_high = *emitted & mask != 0;
            if high == was_high {
                continue;
            }
            match set_optional_signal(ctx, *signal, high) {
                Ok(()) => {
                    if high {
                        *emitted |= mask;
                    } else {
                        *emitted &= !mask;
                    }
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error
    }

    /// Re-evaluate IRQ and every clock gate. This runs on every MMIO write,
    /// not just writes that change ENR, so a previous queue-capacity failure
    /// can be retried by a later access.
    fn sync_outputs(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let mut first_error = None;
        let irq = self.state.cir_flags & self.state.cir_enable != 0;
        if irq != self.state.irq_level {
            match set_optional_signal(ctx, self.params.irq, irq) {
                Ok(()) => self.state.irq_level = irq,
                Err(error) => first_error = Some(error),
            }
        }

        if let Some(error) = Self::deliver_gates(
            ctx,
            &AHBENR_GATES,
            self.state.ahbenr,
            &mut self.emitted_ahbenr,
        ) {
            first_error.get_or_insert(error);
        }
        if let Some(error) = Self::deliver_gates(
            ctx,
            &APB1ENR_GATES,
            self.state.apb1enr,
            &mut self.emitted_apb1enr,
        ) {
            first_error.get_or_insert(error);
        }
        if let Some(error) = Self::deliver_gates(
            ctx,
            &APB2ENR_GATES,
            self.state.apb2enr,
            &mut self.emitted_apb2enr,
        ) {
            first_error.get_or_insert(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn read_word(&self, offset: u64) -> u32 {
        match offset {
            0x00 => self.cr_read(),
            // CFGR: SWS ビット [3:2] は実際の選択状態。
            0x04 => (self.state.cfgr & !0b1100) | (self.state.sws << 2),
            0x08 => self.state.cir_flags | (self.state.cir_enable << 8),
            0x0C => self.state.apb2rstr,
            0x10 => self.state.apb1rstr,
            0x14 => self.state.ahbenr,
            0x18 => self.state.apb2enr,
            0x1C => self.state.apb1enr,
            0x20 => self.state.bdcr,
            0x24 => self.state.csr,
            _ => 0,
        }
    }

    fn write_word(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {
                // 書き込み可能: HSITRIM[7:3] / HSEON / HSEBYP / CSSON / PLLON。
                // HSION はリセット後も常に 1 を維持する簡略化 (無効化は
                // システム停止を意味するため)。
                const WRITABLE: u32 =
                    (1 << 0) | (0x1F << 3) | (1 << 16) | (1 << 18) | (1 << 19) | (1 << 24);
                let old = self.state.cr_writable;
                self.state.cr_writable =
                    (self.state.cr_writable & !WRITABLE) | (value & WRITABLE) | 1;
                self.update_sws();
                self.latch_ready_edges(old, self.state.bdcr, self.state.csr);
            }
            0x04 => {
                // SW/HPRE/PPRE/ADCPRE/PLL 系/USBPRE/MCO を保持。SW への反映は
                // 準備状態次第。
                const WRITABLE: u32 = 0x077F_FFFF;
                self.state.cfgr = (self.state.cfgr & !WRITABLE) | (value & WRITABLE);
                self.update_sws();
            }
            0x08 => {
                // CIR: [15:8] は rw のイネーブル、[31:24] は 1 書き込みで
                // イネーブルを立てる force ビット、[23:16] はフラグのクリア。
                self.state.cir_enable = ((value >> 8) & 0xFF) | ((value >> 24) & 0xFF);
                self.state.cir_flags &= !((value >> 16) & 0xFF);
            }
            0x0C => self.state.apb2rstr = value,
            0x10 => self.state.apb1rstr = value,
            0x14 => {
                self.state.ahbenr = value;
            }
            0x18 => {
                self.state.apb2enr = value;
            }
            0x1C => {
                self.state.apb1enr = value;
            }
            0x20 => {
                let old = self.state.bdcr;
                self.state.bdcr = value & 0x0001_8367;
                self.latch_ready_edges(self.state.cr_writable, old, self.state.csr);
            }
            0x24 => {
                let old = self.state.csr;
                self.state.csr = value & 0xFC01_0003;
                self.latch_ready_edges(self.state.cr_writable, self.state.bdcr, old);
            }
            _ => {}
        }
    }
}

impl MmioTarget for RccF1 {
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
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        Ok(u64::from(self.read_word(offset) & mask))
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
        self.write_word(offset, value as u32);
        self.sync_outputs(ctx).map_err(|_| BusFault::Device)?;
        Ok(())
    }
}

impl EventTarget for RccF1 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;
    use std::vec;

    use super::*;
    use mosaic_core_types::NodeId;

    struct TestCtx {
        edges: Vec<(SignalId, bool)>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self { edges: Vec::new() }
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

    fn read(rcc: &mut RccF1, ctx: &mut TestCtx, offset: u64) -> u32 {
        rcc.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn write(rcc: &mut RccF1, ctx: &mut TestCtx, offset: u64, value: u32) {
        rcc.write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn reset_state_matches_rm0008() {
        let mut rcc = RccF1::new(RccF1Params::new(SignalId(7)));
        let mut ctx = TestCtx::new();
        // CR: HSION|HSIRDY|HSITRIM=16。
        assert_eq!(read(&mut rcc, &mut ctx, 0x00), 0x0000_0083);
        assert_eq!(read(&mut rcc, &mut ctx, 0x04), 0);
        assert_eq!(read(&mut rcc, &mut ctx, 0x18), 0);
        assert_eq!(read(&mut rcc, &mut ctx, 0x1C), 0);
        assert_eq!(rcc.sysclk_hz(), 8_000_000);
        assert_eq!(rcc.hclk_hz(), 8_000_000);
        assert_eq!(rcc.pclk1_hz(), 8_000_000);
        assert_eq!(rcc.pclk2_hz(), 8_000_000);
    }

    #[test]
    fn pll_switch_reports_sws_and_frequencies() {
        let mut rcc = RccF1::new(RccF1Params::new(SignalId(7)));
        let mut ctx = TestCtx::new();
        // HSI/2 = 4 MHz、×16 = 64 MHz。
        write(&mut rcc, &mut ctx, 0x04, 0x0038_0000); // PLLMUL=1110 (×16)
        write(&mut rcc, &mut ctx, 0x00, 0x0100_0081); // PLLON
                                                      // PLLRDY が同時に立つ。
        assert_eq!(read(&mut rcc, &mut ctx, 0x00) & (1 << 25), 1 << 25);
        // まだ SW=HSI。
        assert_eq!(read(&mut rcc, &mut ctx, 0x04) & 0b1100, 0);
        // SW=PLL へ。準備済みなので即座に SWS=10。
        write(&mut rcc, &mut ctx, 0x04, 0x0038_0002);
        let cfgr = read(&mut rcc, &mut ctx, 0x04);
        assert_eq!(cfgr & 0b11, 0b10);
        assert_eq!((cfgr >> 2) & 0b11, 0b10);
        assert_eq!(rcc.sysclk_hz(), 64_000_000);
        // プリスケーラ: HPRE=/2, PPRE1=/4 (PCLK1=8MHz), PPRE2=/1。
        write(&mut rcc, &mut ctx, 0x04, 0x0038_0582);
        assert_eq!(rcc.hclk_hz(), 32_000_000);
        assert_eq!(rcc.pclk1_hz(), 8_000_000);
        assert_eq!(rcc.pclk2_hz(), 32_000_000);
        // PLL 無効化すると SWS は HSI へ戻る (SW が HSI のとき)。
        write(&mut rcc, &mut ctx, 0x00, 0x0000_0081);
        assert_eq!((read(&mut rcc, &mut ctx, 0x04) >> 2) & 0b11, 0);
        assert_eq!(rcc.sysclk_hz(), 8_000_000);
    }

    #[test]
    fn sw_request_to_unready_source_is_ignored() {
        let mut rcc = RccF1::new(RccF1Params::new(SignalId(7)));
        let mut ctx = TestCtx::new();
        // HSE/PLL は無効のまま SW=HSE。
        write(&mut rcc, &mut ctx, 0x04, 0x1);
        assert_eq!((read(&mut rcc, &mut ctx, 0x04) >> 2) & 0b11, 0);
        assert_eq!(rcc.sysclk_hz(), 8_000_000);
    }

    #[test]
    fn enable_registers_drive_gate_signals_on_change() {
        let mut rcc = RccF1::new(RccF1Params::new(SignalId(7)));
        let mut ctx = TestCtx::new();
        // APB2ENR: IOPAEN|USART1EN = 0x4004。
        write(&mut rcc, &mut ctx, 0x18, 0x4004);
        assert_eq!(
            ctx.edges,
            vec![(GATE_APB2_IOPA, true), (GATE_APB2_USART1, true)]
        );
        // 同じ値の再書き込みは追加のエッジを生まない。
        ctx.edges.clear();
        write(&mut rcc, &mut ctx, 0x18, 0x4004);
        assert!(ctx.edges.is_empty());
        // IOPAEN を落とすと立ち下がりエッジ。
        write(&mut rcc, &mut ctx, 0x18, 0x4000);
        assert_eq!(ctx.edges, vec![(GATE_APB2_IOPA, false)]);
        assert_eq!(read(&mut rcc, &mut ctx, 0x18), 0x4000);
        // APB1ENR: I2C1EN。
        ctx.edges.clear();
        write(&mut rcc, &mut ctx, 0x1C, 1 << 21);
        assert_eq!(ctx.edges, vec![(GATE_APB1_I2C1, true)]);
    }

    #[test]
    fn cir_interrupt_output_follows_ready_and_enable() {
        let mut rcc = RccF1::new(RccF1Params::new(SignalId(7)));
        let mut ctx = TestCtx::new();
        // HSEON を立てると HSERDYF (ビット 3) がラッチされる。
        write(&mut rcc, &mut ctx, 0x00, 0x0001_0081); // HSEON
                                                      // HSERDYIE (ビット 11) を立てると出力が立つ。
        write(&mut rcc, &mut ctx, 0x08, 1 << 11);
        assert_eq!(ctx.edges, vec![(SignalId(7), true)]);
        // HSERDYC (ビット 19) の 1 書き込みでフラグが落ち、出力も下がる。
        write(&mut rcc, &mut ctx, 0x08, 1 << 19);
        assert_eq!(ctx.edges.last(), Some(&(SignalId(7), false)));
        // クロックは動いたままなので、再びイネーブルを書いてもフラグは
        // 立たない (エッジラッチ)。
        write(&mut rcc, &mut ctx, 0x08, 1 << 11);
        assert_eq!(ctx.edges.len(), 2);
    }
}
