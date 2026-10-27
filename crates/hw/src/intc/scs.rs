//! ARMv7-M System Control Space (SCS) の機能レジスタモデル。
//!
//! Cortex-M3 クラスのコアが持つ 0xE000_E000–0xE000_EFFF のシステム制御空間を
//! 1 ノードで表す (ARM ARM DDI 0403E; Cortex-M3 の ID 値は DDI 0337 系、
//! QEMU `target/arm/tcg/cpu-v7m.c` cortex_m3_initfn の値と突き合わせ)。
//! 3 つの機能ブロックが同一ノードに住む:
//!
//! - **SysTick** (0x010–0x01F): CSR/RVR/CVR/CALIB。CVR は仮想時間から計算され、
//!   ラップはイベント予約で表現される。CSR 読み出しは COUNTFLAG を消費する
//!   (Attr::kind が Debug の読み出しは消費しない)。
//! - **NVIC** (0x100–0x2FF, 0x400–0x42A): ISER/ICER/ISPR/ICPR/IABR/IPR/STIR。
//!   例外入力 (コアへの配送) は Unicorn 2.1.x に公開 API が無いため行わない
//!   (ADR 0009)。ペンディング状態はレジスタ経由で観測可能。
//!   外部割り込みラインは [`Scs::bind_irq`] で (source node, signal id) →
//!   IRQ 番号を割り当て、[`EventTarget::on_signal`] で源識別付きで届く。
//! - **SCB** (0xD00–0xD7C): CPUID/ICSR/VTOR/AIRCR/SCR/CCR/SHPR/SHCSR/CFSR/HFSR/
//!   MMFAR/BFAR/AFSR と ID レジスタ群 (PFR/DFR/AFR/MMFR/ISAR)。
//!   AIRCR は VECTKEY (0x5FA) を要求する。
//!
//! 未実装の部分 (設計上の境界):
//! - 例外のベクタリング・アクティブ化 (IABR は常に 0、ICSR の VECTACTIVE も 0)。
//! - CFSR/HFSR のフォールト解析ビットは値の保持のみ。
//! - STIR の到達判定は IRQ 番号のみ (9 ビットに切り詰め)。
//! - 予約領域は 0 を返し、書き込みは破棄する (raz-wi)。

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{
    AccessKind, AccessSize, Attr, BusFault, EventId, Fnv64, NodeId, SignalId, Snapshot,
    SnapshotError, VTime,
};

/// SysTick ラップのイベントトークン。
const SYSTICK_TOKEN: u64 = 1;

/// SysTick CSR: ENABLE。
const SYST_ENABLE: u32 = 1 << 0;
/// SysTick CSR: TICKINT。
const SYST_TICKINT: u32 = 1 << 1;
/// SysTick CSR: CLKSOURCE (1 = HCLK、0 = HCLK/8)。
const SYST_CLKSOURCE: u32 = 1 << 2;

/// モデル構成パラメータ。
#[derive(Clone, Copy, Debug)]
pub struct ScsParams {
    /// 実装する外部割り込みライン数。STM32F10x の medium density は 43。
    pub num_irq: usize,
    /// SysTick の参照クロック (HCLK)。CLKSOURCE=1 のときのカウント周期に使う。
    pub hclk_hz: u32,
    /// CALIB レジスタの TENMS 値。STM32F10x は 9000 固定 (RM0008 §19)。
    pub calib_tenms: u32,
}

impl Default for ScsParams {
    fn default() -> Self {
        Self {
            num_irq: 43,
            hclk_hz: 72_000_000,
            calib_tenms: 9000,
        }
    }
}

impl ScsParams {
    pub fn new(num_irq: usize) -> Self {
        Self {
            num_irq,
            ..Self::default()
        }
    }
}

/// Cortex-M3 (r1p1) の ID レジスタ値。QEMU cortex_m3_initfn と同一。
const CPUID: u32 = 0x411F_C231;
const PFR0: u32 = 0x0000_0030;
const PFR1: u32 = 0x0000_0200;
const DFR: u32 = 0x0010_0000;
const AFR: u32 = 0x0000_0000;
const MMFR: [u32; 4] = [0x0000_0030, 0, 0, 0];
const ISAR: [u32; 6] = [
    0x0114_1110,
    0x0211_1000,
    0x2111_2231,
    0x0111_1110,
    0x0131_0102,
    0x0000_0000,
];

/// SysTick の状態。CVR は「基準時刻からの経過時間」から計算する。
#[derive(Clone, Copy, Debug, Default)]
struct SysTick {
    /// ENABLE / TICKINT / CLKSOURCE (COUNTFLAG は別フィールド)。
    ctrl: u32,
    /// RVR。24 ビット。
    reload: u32,
    /// ENABLE=0 の間の固定 CVR 値。
    frozen_cvr: u32,
    /// 最後のラップ (またはイネーブル/リロード時刻) の仮想時間。
    base: VTime,
    /// ラップ未検出フラグ (CSR ビット 16)。
    countflag: bool,
    /// SysTick 例外のペンディング (ICSR ビット 26)。
    pending: bool,
    /// 予約済みタイマーイベント。
    event: Option<EventId>,
}

/// システムハンドラ優先度 (PRI4..PRI15) のバイト列。0xE000ED18 起点 12 バイト、
/// 先頭 1 バイト (0xD18) は予約。
const SHPR_LEN: usize = 12;

#[derive(Clone, Copy, Debug, Default)]
struct Scb {
    /// ICSR の書き込み可能ビット (NMIPENDSET/PENDSVSET) の保持。
    icsr_latched: u32,
    vtor: u32,
    /// AIRCR の PRIGROUP ビット [10:8]。
    prigroup: u32,
    scr: u32,
    ccr: u32,
    /// PRI4..PRI15 の実装バイト。IPR と同じ 4 ビット幅。
    shpr: [u8; SHPR_LEN],
    shcsr: u32,
    cfsr: u32,
    hfsr: u32,
    mmfar: u32,
    bfar: u32,
}

/// ARMv7-M SCS の機能モデル。
pub struct Scs {
    params: ScsParams,
    systick: SysTick,
    /// 外部割り込みのイネーブルビットマップ (32 ビットワード 8 本、ビット n = IRQ n)。
    enable: [u32; 8],
    /// 外部割り込みのペンディングビットマップ。
    pending: [u32; 8],
    /// 優先度レジスタ。IPR はバイト単位、上位 4 ビットのみ実装。
    ipr: [u8; 64],
    scb: Scb,
    /// bind_irq で注入された入力配線 (IRQ 番号 → (source, signal))。
    routing: Vec<Option<(NodeId, SignalId)>>,
}

impl Scs {
    pub fn new(params: ScsParams) -> Self {
        Self {
            params,
            systick: SysTick::default(),
            enable: [0; 8],
            pending: [0; 8],
            ipr: [0; 64],
            scb: Scb::default(),
            routing: vec![None; params.num_irq],
        }
    }

    /// 外部割り込み入力を配線する: `source` ノードの `signal` を IRQ `position` へ。
    /// PL190 の `bind_input` と同じ ADR 0008 ポート注入方式。
    pub fn bind_irq(
        &mut self,
        position: usize,
        source: NodeId,
        signal: SignalId,
    ) -> Result<(), String> {
        if position >= self.params.num_irq {
            return Err(format!("IRQ position {position} out of range"));
        }
        if self.routing[..self.params.num_irq]
            .iter()
            .any(|slot| slot == &Some((source, signal)))
        {
            return Err(format!(
                "signal {signal:?} of node {source:?} already routed"
            ));
        }
        self.routing[position] = Some((source, signal));
        Ok(())
    }

    /// SysTick のカウント周期 (ピコ秒)。
    fn systick_period_ps(&self) -> u64 {
        let hz = if self.systick.ctrl & SYST_CLKSOURCE != 0 {
            self.params.hclk_hz
        } else {
            // CLKSOURCE=0: F1 では HCLK/8 の「外部」クロック。
            self.params.hclk_hz / 8
        };
        if hz == 0 {
            return u64::MAX;
        }
        1_000_000_000_000 / u64::from(hz)
    }

    /// 基準時刻からの経過ティック数。
    fn systick_elapsed(&self, now: VTime) -> u64 {
        let period = self.systick_period_ps();
        if period == u64::MAX {
            return 0;
        }
        now.0
            .checked_sub(self.systick.base.0)
            .map(|delta| delta / period)
            .unwrap_or(0)
    }

    /// 現在の CVR 値 (イネーブル時は計算、無効時は凍結値)。
    fn systick_cvr(&self, now: VTime) -> u32 {
        if self.systick.ctrl & SYST_ENABLE == 0 {
            return self.systick.frozen_cvr;
        }
        let reload = self.systick.reload & 0x00FF_FFFF;
        let full = u64::from(reload) + 1;
        let phase = self.systick_elapsed(now) % full;
        ((full - phase - 1) & 0x00FF_FFFF) as u32
    }

    /// 保留中のラップを消化し、COUNTFLAG/ペンディングを更新する。
    fn systick_absorb(&mut self, now: VTime) {
        let reload = self.systick.reload & 0x00FF_FFFF;
        if self.systick.ctrl & SYST_ENABLE == 0 || reload == 0 {
            return;
        }
        let full = u64::from(reload) + 1;
        let period = self.systick_period_ps();
        if period == u64::MAX {
            return;
        }
        let wraps = self.systick_elapsed(now) / full;
        if wraps == 0 {
            return;
        }
        self.systick.countflag = true;
        if self.systick.ctrl & SYST_TICKINT != 0 {
            self.systick.pending = true;
        }
        // 基準時刻を最後のラップへ進める (余りは位相として保持)。
        self.systick.base = VTime(self.systick.base.0 + wraps * full * period);
    }

    /// SysTick タイマーイベントを予約し直す。
    fn systick_reschedule(&mut self, ctx: &mut dyn Ctx) {
        if let Some(event) = self.systick.event.take() {
            let _ = ctx.cancel(event);
        }
        let reload = self.systick.reload & 0x00FF_FFFF;
        if self.systick.ctrl & SYST_ENABLE == 0 || reload == 0 {
            return;
        }
        let full = u64::from(reload) + 1;
        let period = self.systick_period_ps();
        if period == u64::MAX {
            return;
        }
        let next_wrap_ps = (full - self.systick_elapsed(ctx.now()) % full) * period;
        if let Ok(event) = ctx.schedule(
            VTime(ctx.now().0.saturating_add(next_wrap_ps)),
            SYSTICK_TOKEN,
        ) {
            self.systick.event = Some(event);
        }
    }

    /// 有効かつペンディングな外部割り込みのうち最小の IRQ 番号。
    fn first_enabled_pending(&self) -> Option<usize> {
        for word in 0..self.enable.len() {
            let bits = self.pending[word] & self.enable[word];
            if bits != 0 {
                return Some(word * 32 + bits.trailing_zeros() as usize);
            }
        }
        None
    }

    /// ICSR の読み出し値。
    fn icsr_read(&self) -> u32 {
        let mut value = self.scb.icsr_latched & 0x9000_0000;
        if self.systick.pending {
            value |= 1 << 26;
        }
        // VECTPENDING: 外部割り込み (位置 + 16)、なければ SysTick (15)。
        if let Some(position) = self.first_enabled_pending() {
            value |= 1 << 22; // ISRPENDING
            value |= ((position + 16) as u32) << 12;
        } else if self.systick.pending {
            value |= 1 << 22;
            value |= 15 << 12;
        }
        value
    }

    /// IPR 領域 (0x400..0x440) のオフセット `lane` からの 4 バイトを組み立てる。
    fn ipr_word(&self, offset: u64) -> u32 {
        let start = (offset - 0x400) as usize;
        let mut word = 0u32;
        for i in 0..4 {
            let index = start + i;
            let byte = self.ipr.get(index).copied().unwrap_or(0);
            word |= u32::from(byte) << (8 * i);
        }
        word
    }

    /// IPR 領域へ size 幅で書き込む (バイトレーンのみ更新)。
    fn ipr_write(&mut self, offset: u64, size: AccessSize, value: u32) {
        let start = (offset - 0x400) as usize;
        for i in 0..size.bytes() as usize {
            let byte = ((value >> (8 * i)) as u8) & 0xF0;
            if let Some(slot) = self.ipr.get_mut(start + i) {
                *slot = byte;
            }
        }
    }

    /// SHPR 領域 (0xD18..0xD24) のオフセットからの 4 バイトを組み立てる。
    /// 予約バイト (0xD18) と範囲外は 0。
    fn shpr_word(&self, offset: u64) -> u32 {
        let start = (offset - 0xD18) as usize;
        let mut word = 0u32;
        for i in 0..4 {
            let index = start + i;
            let byte = match self.scb.shpr.get(index) {
                Some(byte) if index != 0 => *byte,
                _ => 0,
            };
            word |= u32::from(byte) << (8 * i);
        }
        word
    }

    /// SHPR 領域へ size 幅で書き込む (予約バイトと範囲外を除く)。
    fn shpr_write(&mut self, offset: u64, size: AccessSize, value: u32) {
        let start = (offset - 0xD18) as usize;
        for i in 0..size.bytes() as usize {
            let index = start + i;
            if index == 0 || index >= SHPR_LEN {
                continue;
            }
            if let Some(slot) = self.scb.shpr.get_mut(index) {
                *slot = ((value >> (8 * i)) as u8) & 0xF0;
            }
        }
    }

    fn read_word(&mut self, ctx: &mut dyn Ctx, offset: u64) -> u32 {
        let now = ctx.now();
        if (0x100..0x120).contains(&offset) {
            // ISER
            let index = ((offset - 0x100) / 4) as usize;
            self.enable[index] & self.irq_word_mask(index)
        } else if (0x180..0x1A0).contains(&offset) {
            // ICER はイネーブル状態を読み出す。
            let index = ((offset - 0x180) / 4) as usize;
            self.enable[index] & self.irq_word_mask(index)
        } else if (0x200..0x220).contains(&offset) {
            // ISPR
            let index = ((offset - 0x200) / 4) as usize;
            self.pending[index] & self.irq_word_mask(index)
        } else if (0x280..0x2A0).contains(&offset) {
            // ICPR はペンディング状態を読み出す。
            let index = ((offset - 0x280) / 4) as usize;
            self.pending[index] & self.irq_word_mask(index)
        } else if (0x300..0x320).contains(&offset) {
            // IABR: コアへの配送が無いため常に 0。
            0
        } else if (0x400..0x440).contains(&offset) {
            self.ipr_word(offset)
        } else if (0xD18..0xD24).contains(&offset) {
            self.shpr_word(offset)
        } else {
            match offset {
                // ICTR: (num_irq + 31) / 32 - 1 (QEMU と同じ式)。
                0x004 => (((self.params.num_irq + 31) / 32) as u32 - 1) & 0xF,
                0x010 => {
                    let mut value = self.systick.ctrl & 0b111;
                    if self.systick.countflag {
                        value |= 1 << 16;
                    }
                    value
                }
                0x014 => self.systick.reload & 0x00FF_FFFF,
                0x018 => self.systick_cvr(now),
                0x01C => self.params.calib_tenms & 0x00FF_FFFF,
                0xD00 => CPUID,
                0xD04 => self.icsr_read(),
                0xD08 => self.scb.vtor & 0x3FFF_FF80,
                0xD0C => 0xFA05_0000 | (self.scb.prigroup << 8),
                0xD10 => self.scb.scr & 0xF,
                0xD14 => self.scb.ccr & 0x0203,
                0xD24 => self.scb.shcsr & 0x0000_FFC0,
                0xD28 => self.scb.cfsr,
                0xD2C => self.scb.hfsr & 0xC000_0003,
                0xD34 => self.scb.mmfar,
                0xD38 => self.scb.bfar,
                0xD3C => 0, // AFSR: 実装定義。実装なし。
                0xD40 => PFR0,
                0xD44 => PFR1,
                0xD48 => DFR,
                0xD4C => AFR,
                0xD50 => MMFR[0],
                0xD54 => MMFR[1],
                0xD58 => MMFR[2],
                0xD5C => MMFR[3],
                0xD60 => ISAR[0],
                0xD64 => ISAR[1],
                0xD68 => ISAR[2],
                0xD6C => ISAR[3],
                0xD70 => ISAR[4],
                0xD74 => ISAR[5],
                _ => 0, // 予約領域は raz。
            }
        }
    }

    /// IRQ ワード `index` の実装ビットマスク。
    fn irq_word_mask(&self, index: usize) -> u32 {
        let remaining = self.params.num_irq.saturating_sub(index * 32);
        if remaining >= 32 {
            u32::MAX
        } else {
            (1u32 << remaining) - 1
        }
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, size: AccessSize, value: u32) {
        if (0x400..0x440).contains(&offset) {
            self.ipr_write(offset, size, value);
            return;
        }
        if (0xD18..0xD24).contains(&offset) {
            self.shpr_write(offset, size, value);
            return;
        }
        let now = ctx.now();
        match offset {
            0x010 => {
                // ENABLE/TICKINT/CLKSOURCE のみ書き込み可能。
                let was_enabled = self.systick.ctrl & SYST_ENABLE != 0;
                let new_ctrl = value & 0b111;
                let now_enabled = new_ctrl & SYST_ENABLE != 0;
                if was_enabled && !now_enabled {
                    self.systick.frozen_cvr = self.systick_cvr(now);
                }
                if !was_enabled && now_enabled {
                    self.systick.base = now;
                }
                self.systick.ctrl = new_ctrl;
                self.systick_reschedule(ctx);
            }
            0x014 => {
                self.systick.reload = value & 0x00FF_FFFF;
                self.systick_reschedule(ctx);
            }
            0x018 => {
                // 任意値の書き込みでカウンタと COUNTFLAG をクリア。
                if self.systick.ctrl & SYST_ENABLE != 0 {
                    self.systick.base = now;
                }
                self.systick.frozen_cvr = 0;
                self.systick.countflag = false;
                self.systick_reschedule(ctx);
            }
            0x100..0x120 => {
                // ISER: 1 を書いたビットのみセット。
                let index = ((offset - 0x100) / 4) as usize;
                let bits = value & self.irq_word_mask(index);
                self.enable[index] |= bits;
            }
            0x180..0x1A0 => {
                // ICER: 1 を書いたビットのみクリア。
                let index = ((offset - 0x180) / 4) as usize;
                let bits = value & self.irq_word_mask(index);
                self.enable[index] &= !bits;
            }
            0x200..0x220 => {
                // ISPR: 1 を書いたビットのみセット。
                let index = ((offset - 0x200) / 4) as usize;
                let bits = value & self.irq_word_mask(index);
                self.pending[index] |= bits;
            }
            0x280..0x2A0 => {
                // ICPR: 1 を書いたビットのみクリア。
                let index = ((offset - 0x280) / 4) as usize;
                let bits = value & self.irq_word_mask(index);
                self.pending[index] &= !bits;
            }
            0xD04 => {
                // ICSR: PENDSVSET/CLR, NMIPENDSET, PENDSTSET/CLR。
                let mut latched = self.scb.icsr_latched & 0x9000_0000;
                if value & (1 << 28) != 0 {
                    latched |= 1 << 28;
                }
                if value & (1 << 27) != 0 {
                    latched &= !(1 << 28);
                }
                if value & (1 << 31) != 0 {
                    latched |= 1 << 31;
                }
                self.scb.icsr_latched = latched;
                if value & (1 << 26) != 0 {
                    self.systick.pending = true;
                }
                if value & (1 << 25) != 0 {
                    self.systick.pending = false;
                }
            }
            0xD08 => {
                self.scb.vtor = value & 0x3FFF_FF80;
            }
            0xD0C => {
                // VECTKEY = 0x5FA が必要。それ以外は破棄。
                if value >> 16 == 0x5FA {
                    self.scb.prigroup = (value >> 8) & 0b111;
                }
            }
            0xD10 => {
                self.scb.scr = value & 0xF;
            }
            0xD14 => {
                self.scb.ccr = value & 0x0203;
            }
            0xD24 => {
                self.scb.shcsr = value & 0x0000_FFC0;
            }
            0xD28 => {
                self.scb.cfsr = value;
            }
            0xD2C => {
                self.scb.hfsr = value & 0xC000_0003;
            }
            0xD34 => {
                self.scb.mmfar = value;
            }
            0xD38 => {
                self.scb.bfar = value;
            }
            0xE00 => {
                // STIR: IRQ 番号 (0 基点) のペンディングを立てる。
                let irq = (value & 0x1FF) as usize;
                if irq < self.params.num_irq {
                    self.pending[irq / 32] |= 1u32 << (irq % 32);
                }
            }
            _ => {
                // 予約領域は wi。
            }
        }
    }
}

impl MmioTarget for Scs {
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
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(ctx, offset);
        // SysTick CSR の COUNTFLAG は読み出しで消費する。ただし Debug 参照は
        // 状態を変えない (MmioTarget 契約)。
        if offset == 0x010 && attr.kind != AccessKind::Debug {
            self.systick.countflag = false;
        }
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
        let _ = attr;
        if matches!(size, AccessSize::DoubleWord) {
            return Err(BusFault::InvalidSize);
        }
        if value > u64::from(u32::MAX) {
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, size, value as u32);
        Ok(())
    }
}

impl EventTarget for Scs {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token != SYSTICK_TOKEN {
            return Ok(());
        }
        self.systick_absorb(ctx.now());
        self.systick_reschedule(ctx);
        Ok(())
    }

    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        source: NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        // 配線された入力だけを受け入れる。レベルトリガの近似として、
        // 立ち上がりでペンディングを立て、立ち下がりで落とす
        // (ドライバのステップ境界でのサンプリング相当)。
        if let Some(position) = self.routing[..self.params.num_irq]
            .iter()
            .position(|slot| slot == &Some((source, signal)))
        {
            let bit = 1u32 << (position % 32);
            if high {
                self.pending[position / 32] |= bit;
            } else {
                self.pending[position / 32] &= !bit;
            }
        }
        Ok(())
    }
}

/// SCS checkpoint. IRQ routing is host wiring and is not included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScsCheckpoint {
    pub schema: u32,
    pub systick_ctrl: u32,
    pub systick_reload: u32,
    pub systick_frozen_cvr: u32,
    pub systick_base: VTime,
    pub systick_countflag: bool,
    pub systick_pending: bool,
    pub systick_event: Option<EventId>,
    pub enable: [u32; 8],
    pub pending: [u32; 8],
    pub ipr: [u8; 64],
    pub icsr_latched: u32,
    pub vtor: u32,
    pub prigroup: u32,
    pub scr: u32,
    pub ccr: u32,
    pub shpr: [u8; SHPR_LEN],
    pub shcsr: u32,
    pub cfsr: u32,
    pub hfsr: u32,
    pub mmfar: u32,
    pub bfar: u32,
}

impl Snapshot for Scs {
    type State = ScsCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        ScsCheckpoint {
            schema: Self::SCHEMA_VERSION,
            systick_ctrl: self.systick.ctrl,
            systick_reload: self.systick.reload,
            systick_frozen_cvr: self.systick.frozen_cvr,
            systick_base: self.systick.base,
            systick_countflag: self.systick.countflag,
            systick_pending: self.systick.pending,
            systick_event: self.systick.event,
            enable: self.enable,
            pending: self.pending,
            ipr: self.ipr,
            icsr_latched: self.scb.icsr_latched,
            vtor: self.scb.vtor,
            prigroup: self.scb.prigroup,
            scr: self.scb.scr,
            ccr: self.scb.ccr,
            shpr: self.scb.shpr,
            shcsr: self.scb.shcsr,
            cfsr: self.scb.cfsr,
            hfsr: self.scb.hfsr,
            mmfar: self.scb.mmfar,
            bfar: self.scb.bfar,
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
        self.systick.ctrl = state.systick_ctrl;
        self.systick.reload = state.systick_reload;
        self.systick.frozen_cvr = state.systick_frozen_cvr;
        self.systick.base = state.systick_base;
        self.systick.countflag = state.systick_countflag;
        self.systick.pending = state.systick_pending;
        self.systick.event = state.systick_event;
        self.enable = state.enable;
        self.pending = state.pending;
        self.ipr = state.ipr;
        self.scb.icsr_latched = state.icsr_latched;
        self.scb.vtor = state.vtor;
        self.scb.prigroup = state.prigroup;
        self.scb.scr = state.scr;
        self.scb.ccr = state.ccr;
        self.scb.shpr = state.shpr;
        self.scb.shcsr = state.shcsr;
        self.scb.cfsr = state.cfsr;
        self.scb.hfsr = state.hfsr;
        self.scb.mmfar = state.mmfar;
        self.scb.bfar = state.bfar;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        hash.u32(state.systick_ctrl);
        hash.u32(state.systick_reload);
        hash.u32(state.systick_frozen_cvr);
        hash.u64(state.systick_base.0);
        hash.bool(state.systick_countflag);
        hash.bool(state.systick_pending);
        hash.option_u64(state.systick_event.map(|id| id.0));
        for word in state.enable {
            hash.u32(word);
        }
        for word in state.pending {
            hash.u32(word);
        }
        hash.bytes(&state.ipr);
        hash.u32(state.icsr_latched);
        hash.u32(state.vtor);
        hash.u32(state.prigroup);
        hash.u32(state.scr);
        hash.u32(state.ccr);
        hash.bytes(&state.shpr);
        hash.u32(state.shcsr);
        hash.u32(state.cfsr);
        hash.u32(state.hfsr);
        hash.u32(state.mmfar);
        hash.u32(state.bfar);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;

    struct TestCtx {
        now: VTime,
        scheduled: Vec<(VTime, u64)>,
    }

    impl TestCtx {
        fn at(ps: u64) -> Self {
            Self {
                now: VTime(ps),
                scheduled: Vec::new(),
            }
        }
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            self.now
        }
        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            self.scheduled.push((at, token));
            Ok(EventId(self.scheduled.len() as u64))
        }
        fn cancel(&mut self, _event: EventId) -> bool {
            false
        }
        fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
            Ok(())
        }
    }

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: mosaic_exec::Security::Secure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: mosaic_exec::Endian::Little,
        }
    }

    fn read(scs: &mut Scs, ctx: &mut TestCtx, offset: u64) -> u32 {
        scs.read(ctx, offset, AccessSize::Word, &attr()).unwrap() as u32
    }

    fn read_b(scs: &mut Scs, ctx: &mut TestCtx, offset: u64) -> u32 {
        scs.read(ctx, offset, AccessSize::Byte, &attr()).unwrap() as u32
    }

    fn write(scs: &mut Scs, ctx: &mut TestCtx, offset: u64, value: u32) {
        scs.write(ctx, offset, AccessSize::Word, u64::from(value), &attr())
            .unwrap();
    }

    fn write_b(scs: &mut Scs, ctx: &mut TestCtx, offset: u64, value: u32) {
        scs.write(ctx, offset, AccessSize::Byte, u64::from(value), &attr())
            .unwrap();
    }

    #[test]
    fn id_registers_match_cortex_m3() {
        let mut scs = Scs::new(ScsParams::default());
        let mut ctx = TestCtx::at(0);
        assert_eq!(read(&mut scs, &mut ctx, 0xD00), 0x411F_C231);
        assert_eq!(read(&mut scs, &mut ctx, 0xD40), 0x0000_0030);
        assert_eq!(read(&mut scs, &mut ctx, 0xD48), 0x0010_0000);
        assert_eq!(read(&mut scs, &mut ctx, 0xD60), 0x0114_1110);
        assert_eq!(read(&mut scs, &mut ctx, 0xD74), 0x0000_0000);
        // ICTR: 43 IRQ -> 1 ワード組。
        assert_eq!(read(&mut scs, &mut ctx, 0x004), 1);
    }

    #[test]
    fn nvic_set_clear_pending_and_priority() {
        let mut scs = Scs::new(ScsParams::default());
        let mut ctx = TestCtx::at(0);
        // ISER1 のビット 5 (IRQ 37 = USART1) を有効化。
        write(&mut scs, &mut ctx, 0x104, 1 << 5);
        assert_eq!(read(&mut scs, &mut ctx, 0x104), 1 << 5);
        // ISER は 0 書き込みで変化しない。
        write(&mut scs, &mut ctx, 0x104, 0);
        assert_eq!(read(&mut scs, &mut ctx, 0x104), 1 << 5);
        // ICER はイネーブル状態を読み出す。
        assert_eq!(read(&mut scs, &mut ctx, 0x184), 1 << 5);
        // STIR で IRQ 37 をソフトトリガ。
        write(&mut scs, &mut ctx, 0xE00, 37);
        assert_eq!(read(&mut scs, &mut ctx, 0x204), 1 << 5);
        // ICSR の VECTPENDING = 37 + 16 = 53。
        let icsr = read(&mut scs, &mut ctx, 0xD04);
        assert_eq!((icsr >> 12) & 0x3FF, 53);
        assert_eq!(icsr & (1 << 22), 1 << 22);
        // ICPR でクリア。
        write(&mut scs, &mut ctx, 0x284, 1 << 5);
        assert_eq!(read(&mut scs, &mut ctx, 0x204), 0);
        // 実装内の IRQ 番号 42 は ISPR1 (0x204) のビット 10。
        write(&mut scs, &mut ctx, 0xE00, 42);
        assert_eq!(read(&mut scs, &mut ctx, 0x204), 1 << 10);
        // 実装外の IRQ 番号への STIR は無視。
        write(&mut scs, &mut ctx, 0xE00, 60);
        assert_eq!(read(&mut scs, &mut ctx, 0x204), 1 << 10);
        // IABR は常に 0 (コアへの配送なし)。
        assert_eq!(read(&mut scs, &mut ctx, 0x300), 0);
        // IPR はバイトアクセスで、上位 4 ビットのみ実装。
        write_b(&mut scs, &mut ctx, 0x400 + 37, 0xFF);
        assert_eq!(read_b(&mut scs, &mut ctx, 0x400 + 37), 0xF0);
        // ワード読み出しで 4 バイト結合 (リトルエンディアン)。
        // IRQ37 はワード 0x424 (IRQ 36..39) のバイト 1。
        write_b(&mut scs, &mut ctx, 0x401, 0x20);
        assert_eq!(read(&mut scs, &mut ctx, 0x400), 0x0000_2000);
        assert_eq!(read(&mut scs, &mut ctx, 0x424), 0x0000_F000);
    }

    #[test]
    fn aircr_requires_vectkey() {
        let mut scs = Scs::new(ScsParams::default());
        let mut ctx = TestCtx::at(0);
        write(&mut scs, &mut ctx, 0xD0C, 0xFA05_0300);
        assert_eq!(read(&mut scs, &mut ctx, 0xD0C) & 0x700, 0);
        // VECTKEY は [31:16] = 0x5FA (0x05FA_0000)。
        write(&mut scs, &mut ctx, 0xD0C, 0x05FA_0300);
        assert_eq!(read(&mut scs, &mut ctx, 0xD0C), 0xFA05_0300);
    }

    #[test]
    fn systick_counts_down_in_virtual_time() {
        let mut scs = Scs::new(ScsParams {
            hclk_hz: 72_000_000,
            ..ScsParams::default()
        });
        let mut ctx = TestCtx::at(0);
        // RVR=3、ENABLE|CLKSOURCE (HCLK 72MHz → 1 ティック 13888ps)。
        write(&mut scs, &mut ctx, 0x014, 3);
        write(&mut scs, &mut ctx, 0x010, SYST_ENABLE | SYST_CLKSOURCE);
        assert_eq!(read(&mut scs, &mut ctx, 0x018), 3);
        // CALIB は TENMS 既定値。
        assert_eq!(read(&mut scs, &mut ctx, 0x01C), 9000);
        // 4 ティック = 55552ps で 1 ラップ。CVR は位相 0 → 3。
        ctx.now = VTime(4 * 13_888);
        assert_eq!(read(&mut scs, &mut ctx, 0x018), 3);
        // 途中の位相も 1 ティック → 2。
        ctx.now = VTime(5 * 13_888);
        assert_eq!(read(&mut scs, &mut ctx, 0x018), 2);
        // ラップは on_timer で消化され、COUNTFLAG が立つ。
        scs.on_timer(&mut ctx, SYSTICK_TOKEN).unwrap();
        assert_eq!(read(&mut scs, &mut ctx, 0x010) & (1 << 16), 1 << 16);
        // CSR 読み出しで COUNTFLAG は消費される。
        assert_eq!(read(&mut scs, &mut ctx, 0x010) & (1 << 16), 0);
        // もう一度ラップさせ、Debug 参照が消費しないことを確認する。
        ctx.now = VTime(9 * 13_888);
        scs.on_timer(&mut ctx, SYSTICK_TOKEN).unwrap();
        let debug = Attr {
            kind: AccessKind::Debug,
            ..attr()
        };
        let _ = scs.read(&mut ctx, 0x010, AccessSize::Word, &debug);
        assert_eq!(read(&mut scs, &mut ctx, 0x010) & (1 << 16), 1 << 16);
        // CVR 書き込みでカウンタと COUNTFLAG をクリア。
        write(&mut scs, &mut ctx, 0x018, 0xFFFF_FFFF);
        assert_eq!(read(&mut scs, &mut ctx, 0x010) & (1 << 16), 0);
        assert_eq!(read(&mut scs, &mut ctx, 0x018), 3);
        // TICKINT ありでラップ → SysTick ペンディング (ICSR ビット 26)。
        write(
            &mut scs,
            &mut ctx,
            0x010,
            SYST_ENABLE | SYST_CLKSOURCE | SYST_TICKINT,
        );
        ctx.now = VTime(13 * 13_888);
        scs.on_timer(&mut ctx, SYSTICK_TOKEN).unwrap();
        let icsr = read(&mut scs, &mut ctx, 0xD04);
        assert_eq!(icsr & (1 << 26), 1 << 26);
        // VECTPENDING は SysTick (15)。
        assert_eq!((icsr >> 12) & 0x3FF, 15);
    }

    #[test]
    fn wired_irq_input_sets_and_clears_pending() {
        let mut scs = Scs::new(ScsParams::default());
        let source = NodeId(3);
        let signal = SignalId(2);
        scs.bind_irq(6, source, signal).unwrap();
        let mut ctx = TestCtx::at(0);
        scs.on_signal(&mut ctx, source, signal, true).unwrap();
        assert_eq!(read(&mut scs, &mut ctx, 0x200), 1 << 6);
        scs.on_signal(&mut ctx, source, signal, false).unwrap();
        assert_eq!(read(&mut scs, &mut ctx, 0x200), 0);
        // 未配線の入力は無視。
        scs.on_signal(&mut ctx, NodeId(9), SignalId(2), true)
            .unwrap();
        assert_eq!(read(&mut scs, &mut ctx, 0x200), 0);
        // 二重配線は拒否。
        assert!(scs.bind_irq(7, source, signal).is_err());
        assert!(scs.bind_irq(43, NodeId(4), SignalId(1)).is_err());
    }

    #[test]
    fn vtor_and_shpr_storage() {
        let mut scs = Scs::new(ScsParams::default());
        let mut ctx = TestCtx::at(0);
        write(&mut scs, &mut ctx, 0xD08, 0x0800_0000 | 0x7F);
        assert_eq!(read(&mut scs, &mut ctx, 0xD08), 0x0800_0000);
        // SysTick 優先度 = 0xE000ED22 (SHPR3 のバイト 2)。
        write_b(&mut scs, &mut ctx, 0xD22, 0xC0);
        assert_eq!(read_b(&mut scs, &mut ctx, 0xD22), 0xC0);
        // 予約バイト 0xD18 は書けない。
        write_b(&mut scs, &mut ctx, 0xD18, 0xFF);
        assert_eq!(read_b(&mut scs, &mut ctx, 0xD18), 0);
    }

    #[test]
    fn reserved_regions_are_raz_wi() {
        let mut scs = Scs::new(ScsParams::default());
        let mut ctx = TestCtx::at(0);
        assert_eq!(read(&mut scs, &mut ctx, 0x0), 0);
        assert_eq!(read(&mut scs, &mut ctx, 0x2FF), 0);
        write(&mut scs, &mut ctx, 0x0, 0xFFFF_FFFF);
        assert_eq!(read(&mut scs, &mut ctx, 0x0), 0);
    }
}

#[cfg(test)]
mod snapshot_roundtrip {
    use mosaic_exec::{EventId, Snapshot, VTime};

    use super::*;

    #[test]
    fn systick_reservation_and_nvic_masks_restore() {
        let mut scs = Scs::new(ScsParams::default());
        scs.systick.reload = 0x00FF;
        scs.systick.event = Some(EventId(4));
        scs.systick.base = VTime(1_000);
        scs.enable[0] = 0x8;
        scs.scb.vtor = 0x0800_0000;
        let saved = scs.save();
        scs.systick.event = None;
        scs.enable[0] = 0;
        scs.scb.vtor = 0;
        scs.load(Scs::SCHEMA_VERSION, &saved).unwrap();
        assert_eq!(scs.systick.reload, 0x00FF);
        assert_eq!(scs.systick.event, Some(EventId(4)));
        assert_eq!(scs.systick.base, VTime(1_000));
        assert_eq!(scs.enable[0], 0x8);
        assert_eq!(scs.scb.vtor, 0x0800_0000);
    }
}
