//! ARM Generic Interrupt Controller v3 (GICv3) — distributor +
//! redistributor frames.
//!
//! Clean-room from ARM IHI 0069 (architecture specification), with the
//! register offsets and CTLR bit layout cross-checked against QEMU's
//! `hw/intc/gicv3_internal.h`. One node decodes one contiguous window:
//!
//! * `0x0000..0x10000` — the distributor (GICD) frame;
//! * `0x10000 + n*0x20000` — redistributor `n`'s RD_base frame, with its
//!   SGI_base frame at `+0x10000` (the standard two-page-per-CPU layout).
//!
//! Banked state layout (SGIs/PPIs are banked per CPU, SPIs are shared):
//! the enable/pending/active/group words are `[cpu0 bank, cpu1 bank, ...,
//! SPI words...]`; priorities are `[cpu0 32 bytes, cpu1 32 bytes, ...,
//! SPI bytes...]`; ICFGR words are `[cpu0 sgi, cpu0 ppi, cpu1 sgi, cpu1
//! ppi, ..., SPI words...]`. GICD register views of the banked words show
//! cpu 0's bank, which is the architectural "current processor" view of a
//! single-cluster system addressed through the distributor.
//!
//! What is modelled (the single-cluster, no-LPI slice a machine can drive
//! to an external IRQ line):
//!
//! * GICD_CTLR (Grp0/Grp1NS/Grp1S enables, ARE, DS; RWP reads 0 — every
//!   write completes immediately), GICD_TYPER (ITLinesNumber from the
//!   configured SPI count, CPU number, no LPIS), GICD_IIDR;
//! * the full GICD enable/pending/active/group/priority/config blocks,
//!   with ITARGETSR RAZ/WI as v3 requires (routing is GICD_IROUTER,
//!   stored and honoured per interrupt);
//! * GICD_SETSPI_NSR/CLRSPI_NSR (forwarded software-generated SPIs) and
//!   GICD_SGIR with its target-list/filter semantics;
//! * each redistributor's GICR_CTLR/TYPER/IIDR/WAKER (ProcessorSleep and
//!   ChildrenAsleep track each other; a sleeping redistributor forces its
//!   outputs low) and the SGI_base's IGROUPR0/ISENABLER0/ICENABLER0/
//!   ISPENDR0/ICPENDR0/ISACTIVER0/ICACTIVER0/IPRIORITYR/ICFGR0/ICFGR1;
//! * the peripheral ID registers both frames answer (PIDR2 = 0x20: the
//!   GICv3 architecture revision, which is how Linux's driver identifies
//!   a v3).
//!
//! Architectural boundary, stated rather than hidden: the GICv3 CPU
//! interface is a *system-register* interface (ICC_IAR0/1_EL1,
//! ICC_EOIR0/1_EL1, ICC_PMR_EL1, ICC_IGRPEN*), and Unicorn 2.1.x neither
//! implements those registers nor injects external IRQs (ADR 0009). This
//! model therefore drives one IRQ output line per CPU (Group 1) and one
//! FIQ line (Group 0) and exposes `acknowledge`/`eoi` as ordinary methods
//! for machine glue and tests. LPIs (the ITS) are out of scope:
//! GICD_TYPER.LPIS reads 0 and the LPI registers are RAZ/WI.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

/// Size of the distributor frame.
pub const GICD_FRAME: u64 = 0x1_0000;
/// Stride of one redistributor (RD_base + SGI_base).
pub const GICR_STRIDE: u64 = 0x2_0000;
/// Offset of the SGI_base frame inside one redistributor.
pub const GICR_SGI_OFFSET: u64 = 0x1_0000;
/// Default number of SPIs (interrupt IDs 32..=255).
pub const DEFAULT_SPIS: u32 = 224;

// Register constants (IHI 0069 / QEMU gicv3_internal.h).
pub const GICD_CTLR_EN_GRP0: u32 = 1 << 0;
pub const GICD_CTLR_EN_GRP1NS: u32 = 1 << 1;
pub const GICD_CTLR_EN_GRP1S: u32 = 1 << 2;
pub const GICD_CTLR_ARE: u32 = 1 << 4;
pub const GICD_CTLR_ARE_NS: u32 = 1 << 5;
pub const GICD_CTLR_DS: u32 = 1 << 6;
pub const GICD_CTLR_E1NWF: u32 = 1 << 7;
pub const GICR_TYPER_LAST: u64 = 1 << 4;
pub const GICR_WAKER_PROCESSOR_SLEEP: u64 = 1 << 1;
pub const GICR_WAKER_CHILDREN_ASLEEP: u64 = 1 << 2;
/// IIDR: implementer 0x43b (ARM), architecture 2 (GICv3).
pub const GICD_IIDR: u32 = 0x43B_0002;
pub const GICR_IIDR: u32 = 0x43B_0002;
/// PIDR2: arch revision 2 in bits [7:4].
pub const PIDR2_GICV3: u32 = 0x2 << 4;

/// Configuration of one GICv3 instance.
#[derive(Clone, Copy, Debug)]
pub struct GicV3Params {
    /// SPI count (interrupt IDs above 31).
    pub spis: u32,
    /// Redistributor (CPU) count.
    pub cpus: u32,
}

impl Default for GicV3Params {
    fn default() -> Self {
        Self {
            spis: DEFAULT_SPIS,
            cpus: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GicV3State {
    pub schema: u32,
    /// GICD_CTLR (EN_GRP0 | EN_GRP1NS | EN_GRP1S | ARE | DS).
    pub gicd_ctlr: u32,
    /// `[cpus banks][SPI words]`, 32 interrupt bits per word.
    pub enabled: Vec<u32>,
    /// Same layout, pending bits.
    pub pending: Vec<u32>,
    /// Same layout, active bits.
    pub active: Vec<u32>,
    /// Same layout, group bits (1 = Group 1 / NS).
    pub group: Vec<u32>,
    /// `[cpus × 32 bytes][SPI bytes]`, one priority byte per INTID.
    pub priority: Vec<u8>,
    /// `[cpu0 sgi, cpu0 ppi, cpu1 sgi, cpu1 ppi, ...][SPI words]`.
    pub icfgr: Vec<u32>,
    /// GICD_IROUTER per SPI (affinity routing target, 64-bit).
    pub irouter: Vec<u64>,
    /// GICR_CTLR per redistributor.
    pub gicr_ctlr: Vec<u32>,
    /// GICR_WAKER.ProcessorSleep per redistributor.
    pub gicr_sleep: Vec<bool>,
    /// ICC_PMR_EL1 stand-in per CPU (until system-register access exists).
    pub pmr: Vec<u8>,
    /// ICC_IGRPEN1_EL1 stand-in per CPU.
    pub grp1_enabled: Vec<bool>,
}

pub struct GicV3 {
    state: GicV3State,
    params: GicV3Params,
    /// Group-1 (IRQ) output line per CPU.
    irq_outputs: Vec<Option<SignalId>>,
    /// Group-0 (FIQ) output line per CPU.
    fiq_outputs: Vec<Option<SignalId>>,
}

impl GicV3 {
    pub fn new(params: GicV3Params) -> Self {
        let cpus = params.cpus.max(1) as usize;
        let spis = params.spis.max(1);
        let params = GicV3Params {
            spis,
            cpus: cpus as u32,
        };
        let spi_words = (params.spis as usize).div_ceil(32);
        let ints_word = cpus + spi_words;
        Self {
            state: GicV3State {
                schema: 1,
                gicd_ctlr: 0,
                enabled: vec![0; ints_word],
                pending: vec![0; ints_word],
                active: vec![0; ints_word],
                group: vec![0; ints_word],
                priority: vec![0; cpus * 32 + params.spis as usize],
                // SGI words (fixed 0xAAAA) + one PPI word per CPU + SPI words.
                icfgr: {
                    let mut words = Vec::with_capacity(cpus * 2 + spi_words * 2);
                    for _ in 0..cpus {
                        words.push(0xAAAA); // SGIs: config 0b10, edge
                        words.push(0); // PPIs
                    }
                    words.resize(cpus * 2 + (params.spis as usize).div_ceil(16), 0);
                    words
                },
                irouter: vec![0; params.spis as usize],
                gicr_ctlr: vec![0; cpus],
                gicr_sleep: vec![false; cpus],
                pmr: vec![0; cpus],
                grp1_enabled: vec![false; cpus],
            },
            params,
            irq_outputs: vec![None; cpus],
            fiq_outputs: vec![None; cpus],
        }
    }

    /// Wire cpu `cpu`'s Group-1 IRQ output (machine glue / tests).
    pub fn bind_irq_output(&mut self, cpu: usize, signal: SignalId) {
        if let Some(slot) = self.irq_outputs.get_mut(cpu) {
            *slot = Some(signal);
        }
    }

    /// Wire cpu `cpu`'s Group-0 FIQ output (machine glue / tests).
    pub fn bind_fiq_output(&mut self, cpu: usize, signal: SignalId) {
        if let Some(slot) = self.fiq_outputs.get_mut(cpu) {
            *slot = Some(signal);
        }
    }

    fn cpus(&self) -> usize {
        self.params.cpus.max(1) as usize
    }

    fn int_count(&self) -> u32 {
        32 + self.params.spis
    }

    /// Flat banked-word index of (cpu, intid) in the enabled/pending/
    /// active/group vectors.
    fn word_index(&self, cpu: usize, intid: u32) -> Option<usize> {
        if intid < 32 {
            Some(cpu)
        } else {
            let spi = intid - 32;
            if spi >= self.params.spis {
                None
            } else {
                Some(self.cpus() + (spi / 32) as usize)
            }
        }
    }

    /// Flat byte index of (cpu, intid) in the priority vector.
    fn prio_index(&self, cpu: usize, intid: u32) -> Option<usize> {
        if intid < 32 {
            Some(cpu * 32 + intid as usize)
        } else {
            let spi = intid - 32;
            if spi >= self.params.spis {
                None
            } else {
                Some(self.cpus() * 32 + spi as usize)
            }
        }
    }

    /// Flat word index of (cpu, intid) in the icfgr vector (2 words per
    /// bank: SGIs then PPIs; one word per 16 SPIs).
    fn icfgr_index(&self, cpu: usize, intid: u32) -> Option<usize> {
        if intid < 32 {
            Some(cpu * 2 + (intid / 16) as usize)
        } else {
            let spi = intid - 32;
            if spi >= self.params.spis {
                None
            } else {
                Some(self.cpus() * 2 + (spi / 16) as usize)
            }
        }
    }

    /// The GICD view of a banked word: word 0 is cpu 0's SGI/PPI bank,
    /// words from 1 are the SPI words.
    fn dist_word_view(&self, word: usize) -> Option<usize> {
        Some(if word == 0 { 0 } else { self.cpus() + word - 1 })
    }

    /// GICD_TYPER: ITLinesNumber in [4:0], CPU number in [10:8], LPIS=0.
    fn gicd_typer(&self) -> u32 {
        let lines = self.int_count().div_ceil(32) - 1;
        let cpus = (self.cpus() - 1).min(0xFF) as u32;
        (lines & 0x1F) | (cpus << 8)
    }

    /// Raise or drop an interrupt's pending state (fabric-facing input:
    /// signal number = INTID).
    pub fn set_irq_level(&mut self, ctx: &mut dyn Ctx, intid: u32, high: bool) {
        self.set_irq_level_for(ctx, 0, intid, high);
    }

    /// Per-CPU input: SGIs/PPIs (INTID < 32) are banked; SPIs share one bank.
    pub fn set_irq_level_for(&mut self, ctx: &mut dyn Ctx, cpu: usize, intid: u32, high: bool) {
        if intid >= self.int_count() {
            return;
        }
        let Some(index) = self.word_index(cpu, intid) else {
            return;
        };
        let bit = 1u32 << (intid % 32);
        let edge = self
            .icfgr_index(cpu, intid)
            .and_then(|i| self.state.icfgr.get(i).copied())
            .map(|word| (word >> ((intid % 16) * 2 + 1)) & 1 != 0)
            .unwrap_or(false);
        if high {
            self.state.pending[index] |= bit;
        } else if !edge {
            self.state.pending[index] &= !bit;
        }
        self.update_outputs(ctx);
    }

    /// The CPU a pending SPI targets (IROUTER affinity 0.0.0.<n>; an
    /// unmapped target falls back to cpu 0 — single-cluster scope).
    fn spi_target_cpu(&self, intid: u32) -> usize {
        let route = self.state.irouter[(intid - 32) as usize];
        ((route & 0xFF) as usize).min(self.cpus() - 1)
    }

    /// Highest-priority signalable interrupt of `cpu`: enabled, pending,
    /// not active, priority better than PMR, group enabled. Returns
    /// (INTID, is_group0).
    fn highest_signal(&self, cpu: usize) -> Option<(u32, bool)> {
        let pmr = self.state.pmr.get(cpu).copied().unwrap_or(0);
        let grp1 = self.state.grp1_enabled.get(cpu).copied().unwrap_or(false);
        let ctlr = self.state.gicd_ctlr;
        let mut best: Option<(u8, u32, bool)> = None;
        for intid in 0..self.int_count() {
            if intid >= 32 && self.spi_target_cpu(intid) != cpu {
                continue;
            }
            let Some(index) = self.word_index(cpu, intid) else {
                continue;
            };
            let bit = 1u32 << (intid % 32);
            if self.state.enabled[index] & bit == 0
                || self.state.pending[index] & bit == 0
                || self.state.active[index] & bit != 0
            {
                continue;
            }
            let prio = self
                .prio_index(cpu, intid)
                .and_then(|i| self.state.priority.get(i).copied())
                .unwrap_or(0);
            if prio > pmr {
                continue;
            }
            let group0 = self.state.group[index] & bit == 0;
            let group_enabled = if group0 {
                ctlr & GICD_CTLR_EN_GRP0 != 0
            } else {
                ctlr & GICD_CTLR_EN_GRP1NS != 0 && grp1
            };
            if !group_enabled {
                continue;
            }
            if best.is_none_or(|(bp, _, _)| prio < bp) {
                best = Some((prio, intid, group0));
            }
        }
        best.map(|(_, intid, group0)| (intid, group0))
    }

    fn update_outputs(&self, ctx: &mut dyn Ctx) {
        for cpu in 0..self.cpus() {
            let signal = self.highest_signal(cpu);
            let sleeping = self.state.gicr_sleep.get(cpu).copied().unwrap_or(false);
            if let Some(signal_id) = self.irq_outputs.get(cpu).copied().flatten() {
                let high = matches!(signal, Some((_, false))) && !sleeping;
                let _ = ctx.set_signal(signal_id, high);
            }
            if let Some(signal_id) = self.fiq_outputs.get(cpu).copied().flatten() {
                let high = matches!(signal, Some((_, true))) && !sleeping;
                let _ = ctx.set_signal(signal_id, high);
            }
        }
    }

    /// ICC_IAR1_EL1 stand-in: acknowledge the highest-pending Group-1
    /// interrupt on `cpu`. Returns 1023 (spurious) when nothing qualifies.
    pub fn acknowledge(&mut self, ctx: &mut dyn Ctx, cpu: usize) -> u32 {
        if let Some((intid, false)) = self.highest_signal(cpu) {
            let index = self
                .word_index(cpu, intid)
                .expect("validated in highest_signal");
            let bit = 1u32 << (intid % 32);
            self.state.pending[index] &= !bit;
            self.state.active[index] |= bit;
            self.update_outputs(ctx);
            intid
        } else {
            1023
        }
    }

    /// ICC_EOIR1_EL1 stand-in: deactivate an acknowledged interrupt.
    pub fn end_of_interrupt(&mut self, ctx: &mut dyn Ctx, cpu: usize, intid: u32) {
        let intid = intid & 0x3FF;
        if intid >= self.int_count() {
            return;
        }
        if let Some(index) = self.word_index(cpu, intid) {
            let bit = 1u32 << (intid % 32);
            self.state.active[index] &= !bit;
        }
        self.update_outputs(ctx);
    }

    // ---- frame decode ----------------------------------------------------

    /// Window offset → (redistributor index, offset within its frames);
    /// None means the distributor frame.
    fn redist_of(&self, offset: u64) -> Option<(usize, u64)> {
        if offset < GICD_FRAME {
            return None;
        }
        let rel = offset - GICD_FRAME;
        let cpu = (rel / GICR_STRIDE) as usize;
        if cpu >= self.cpus() {
            return Some((usize::MAX, rel % GICR_STRIDE));
        }
        Some((cpu, rel % GICR_STRIDE))
    }

    // ---- distributor ------------------------------------------------------

    fn dist_read(&mut self, offset: u64, size: AccessSize) -> u64 {
        let block = |base: u64, span: u64| (base..base + span).contains(&offset);
        match offset {
            0x0000 => return u64::from(self.state.gicd_ctlr),
            0x0004 => return u64::from(self.gicd_typer()),
            0x0008 => return u64::from(GICD_IIDR),
            0xFFE8 => return u64::from(PIDR2_GICV3),
            0xFFD0..=0xFFFF => return 0,
            _ => {}
        }
        let banks = [
            (0x0080, &self.state.group),
            (0x0100, &self.state.enabled),
            (0x0180, &self.state.enabled),
            (0x0200, &self.state.pending),
            (0x0280, &self.state.pending),
            (0x0300, &self.state.active),
            (0x0380, &self.state.active),
            (0x0C00, &self.state.icfgr),
        ];
        for (base, bank) in banks {
            if block(base, 0x80) || (base == 0x0C00 && block(base, 0x100)) {
                let w = ((offset - base) / 4) as usize;
                let view = if base == 0x0C00 {
                    // ICFGR: word 0/1 are cpu 0's SGI/PPI words.
                    Some(w)
                } else {
                    self.dist_word_view(w)
                };
                return u64::from(view.and_then(|v| bank.get(v).copied()).unwrap_or(0));
            }
        }
        if block(0x0400, 0x0400) {
            // IPRIORITYR: byte per INTID (cpu 0's bank below 32).
            let intid = ((offset - 0x0400) / size.bytes() as u64 * size.bytes() as u64) as u32;
            let mut value = 0u64;
            for i in 0..size.bytes() as u32 {
                if let Some(idx) = self.prio_index(0, intid + i) {
                    if let Some(byte) = self.state.priority.get(idx) {
                        value |= u64::from(*byte) << (i * 8);
                    }
                }
            }
            return value;
        }
        if block(0x0800, 0x0400) {
            return 0; // ITARGETSR is RAZ in v3
        }
        if block(0x6000, 0x400) && size == AccessSize::DoubleWord {
            let idx = ((offset - 0x6000) / 8) as usize;
            return self.state.irouter.get(idx).copied().unwrap_or(0);
        }
        0
    }

    fn dist_write(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u64, size: AccessSize) {
        let block = |base: u64, span: u64| (base..base + span).contains(&offset);
        match offset {
            0x0000 => {
                self.state.gicd_ctlr = (value as u32)
                    & (GICD_CTLR_EN_GRP0
                        | GICD_CTLR_EN_GRP1NS
                        | GICD_CTLR_EN_GRP1S
                        | GICD_CTLR_ARE
                        | GICD_CTLR_ARE_NS
                        | GICD_CTLR_DS
                        | GICD_CTLR_E1NWF);
                self.update_outputs(ctx);
                return;
            }
            0x0040 | 0x0050 => {
                let intid = (value as u32) & 0x3FF;
                self.set_spi_pending(ctx, intid, true);
                return;
            }
            0x0048 | 0x0058 => {
                let intid = (value as u32) & 0x3FF;
                self.set_spi_pending(ctx, intid, false);
                return;
            }
            0x0F00 => {
                self.dist_sgi(ctx, value as u32);
                return;
            }
            _ => {}
        }
        if block(0x0080, 0x80) {
            let w = ((offset - 0x0080) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.group.get_mut(view) {
                    *word = value as u32;
                }
            }
            return;
        }
        if block(0x0100, 0x80) {
            let w = ((offset - 0x0100) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.enabled.get_mut(view) {
                    *word |= value as u32;
                }
            }
            self.update_outputs(ctx);
            return;
        }
        if block(0x0180, 0x80) {
            let w = ((offset - 0x0180) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.enabled.get_mut(view) {
                    *word &= !(value as u32);
                }
            }
            self.update_outputs(ctx);
            return;
        }
        if block(0x0200, 0x80) {
            let w = ((offset - 0x0200) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.pending.get_mut(view) {
                    *word |= value as u32;
                }
            }
            self.update_outputs(ctx);
            return;
        }
        if block(0x0280, 0x80) {
            let w = ((offset - 0x0280) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.pending.get_mut(view) {
                    *word &= !(value as u32);
                }
            }
            self.update_outputs(ctx);
            return;
        }
        if block(0x0300, 0x80) {
            let w = ((offset - 0x0300) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.active.get_mut(view) {
                    *word |= value as u32;
                }
            }
            return;
        }
        if block(0x0380, 0x80) {
            let w = ((offset - 0x0380) / 4) as usize;
            if let Some(view) = self.dist_word_view(w) {
                if let Some(word) = self.state.active.get_mut(view) {
                    *word &= !(value as u32);
                }
            }
            return;
        }
        if block(0x0400, 0x0400) {
            let intid = ((offset - 0x0400) / size.bytes() as u64 * size.bytes() as u64) as u32;
            for i in 0..size.bytes() as u32 {
                if let Some(idx) = self.prio_index(0, intid + i) {
                    if let Some(byte) = self.state.priority.get_mut(idx) {
                        *byte = ((value >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            self.update_outputs(ctx);
            return;
        }
        if block(0x0800, 0x0400) {
            return; // ITARGETSR is WI in v3
        }
        if block(0x0C00, 0x100) {
            let w = ((offset - 0x0C00) / 4) as usize;
            let sgi_fixed = w % 2 == 0 && w < self.cpus() * 2;
            if let Some(word) = self.state.icfgr.get_mut(w) {
                // SGI config bits are architecturally fixed; word 0 keeps
                // its 0xAAAA pattern.
                if sgi_fixed {
                    *word = 0xAAAA;
                } else {
                    *word = value as u32;
                }
            }
            return;
        }
        if block(0x6000, 0x400) && size == AccessSize::DoubleWord {
            let idx = ((offset - 0x6000) / 8) as usize;
            if let Some(route) = self.state.irouter.get_mut(idx) {
                *route = value;
            }
            self.update_outputs(ctx);
        }
    }

    fn set_spi_pending(&mut self, ctx: &mut dyn Ctx, intid: u32, high: bool) {
        if intid < 32 || intid >= self.int_count() {
            return;
        }
        if let Some(index) = self.word_index(0, intid) {
            let bit = 1u32 << (intid % 32);
            if high {
                self.state.pending[index] |= bit;
            } else {
                self.state.pending[index] &= !bit;
            }
            self.update_outputs(ctx);
        }
    }

    /// GICD_SGIR: SGI `INTID` to the target-list CPUs (filter 0), all
    /// others (1), or just this CPU (2).
    fn dist_sgi(&mut self, ctx: &mut dyn Ctx, value: u32) {
        let intid = value & 0xF;
        let filter = (value >> 24) & 0x3;
        let target_list = (value >> 16) & 0xFF;
        for cpu in 0..self.cpus() {
            let matches = match filter {
                0 => target_list & (1 << cpu) != 0,
                1 => cpu != 0,
                2 => cpu == 0,
                _ => false,
            };
            if !matches {
                continue;
            }
            if let Some(index) = self.word_index(cpu, intid) {
                self.state.pending[index] |= 1 << (intid % 32);
            }
        }
        self.update_outputs(ctx);
    }

    // ---- redistributor ----------------------------------------------------

    fn redist_read(&mut self, cpu: usize, offset: u64, size: AccessSize) -> u64 {
        if cpu == usize::MAX {
            return 0;
        }
        if offset < GICR_SGI_OFFSET {
            return match offset {
                0x0000 => u64::from(self.state.gicr_ctlr.get(cpu).copied().unwrap_or(0)),
                0x0004 => u64::from(GICR_IIDR),
                0x0008 => {
                    let mut value = (cpu as u64) << 8;
                    if cpu + 1 == self.cpus() {
                        value |= GICR_TYPER_LAST;
                    }
                    value
                }
                0x0014 => self.gicr_waker(cpu),
                0x00C0 => 0, // SYNCR: idle
                0xFFE8 => u64::from(PIDR2_GICV3),
                _ => 0,
            };
        }
        let sgi = offset - GICR_SGI_OFFSET;
        let banked = |banks: &[u32]| u64::from(banks.get(cpu).copied().unwrap_or(0));
        if (0x0400..0x0500).contains(&sgi) {
            let intid = ((sgi - 0x0400) / size.bytes() as u64 * size.bytes() as u64) as u32;
            let mut value = 0u64;
            for i in 0..size.bytes() as u32 {
                if let Some(idx) = self.prio_index(cpu, intid + i) {
                    if let Some(byte) = self.state.priority.get(idx) {
                        value |= u64::from(*byte) << (i * 8);
                    }
                }
            }
            return value;
        }
        match sgi {
            0x0080 => banked(&self.state.group),
            0x0100 | 0x0180 => banked(&self.state.enabled),
            0x0200 | 0x0280 => banked(&self.state.pending),
            0x0300 | 0x0380 => banked(&self.state.active),
            0x0C00 => u64::from(self.state.icfgr.get(cpu * 2).copied().unwrap_or(0xAAAA)),
            0x0C04 => u64::from(self.state.icfgr.get(cpu * 2 + 1).copied().unwrap_or(0)),
            _ => 0,
        }
    }

    fn redist_write(
        &mut self,
        ctx: &mut dyn Ctx,
        cpu: usize,
        offset: u64,
        value: u64,
        size: AccessSize,
    ) {
        if cpu == usize::MAX {
            return;
        }
        if offset < GICR_SGI_OFFSET {
            match offset {
                0x0000 => {
                    if let Some(word) = self.state.gicr_ctlr.get_mut(cpu) {
                        // LPI enable bits are stored but inert (no ITS).
                        *word = (value as u32) & 0x0700_0001;
                    }
                }
                0x0014 => {
                    let sleep = value & GICR_WAKER_PROCESSOR_SLEEP != 0;
                    if let Some(slot) = self.state.gicr_sleep.get_mut(cpu) {
                        *slot = sleep;
                    }
                    self.update_outputs(ctx);
                }
                _ => {}
            }
            return;
        }
        let sgi = offset - GICR_SGI_OFFSET;
        let banked = |banks: &mut Vec<u32>, op: &dyn Fn(u32, u32) -> u32| {
            if let Some(word) = banks.get_mut(cpu) {
                *word = op(*word, value as u32);
            }
        };
        if (0x0400..0x0500).contains(&sgi) {
            let intid = ((sgi - 0x0400) / size.bytes() as u64 * size.bytes() as u64) as u32;
            for i in 0..size.bytes() as u32 {
                if let Some(idx) = self.prio_index(cpu, intid + i) {
                    if let Some(byte) = self.state.priority.get_mut(idx) {
                        *byte = ((value >> (i * 8)) & 0xFF) as u8;
                    }
                }
            }
            self.update_outputs(ctx);
            return;
        }
        match sgi {
            0x0080 => banked(&mut self.state.group, &|_old, new| new),
            0x0100 => {
                banked(&mut self.state.enabled, &|old, new| old | new);
                self.update_outputs(ctx);
            }
            0x0180 => {
                banked(&mut self.state.enabled, &|old, new| old & !new);
                self.update_outputs(ctx);
            }
            0x0200 => {
                banked(&mut self.state.pending, &|old, new| old | new);
                self.update_outputs(ctx);
            }
            0x0280 => {
                banked(&mut self.state.pending, &|old, new| old & !new);
                self.update_outputs(ctx);
            }
            0x0300 => banked(&mut self.state.active, &|old, new| old | new),
            0x0380 => banked(&mut self.state.active, &|old, new| old & !new),
            0x0C04 => {
                if let Some(word) = self.state.icfgr.get_mut(cpu * 2 + 1) {
                    *word = value as u32;
                }
            }
            _ => {}
        }
    }

    fn gicr_waker(&self, cpu: usize) -> u64 {
        if self.state.gicr_sleep.get(cpu).copied().unwrap_or(false) {
            GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP
        } else {
            0
        }
    }
}

impl Default for GicV3 {
    fn default() -> Self {
        Self::new(GicV3Params::default())
    }
}

impl MmioTarget for GicV3 {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let value = match self.redist_of(addr) {
            None => self.dist_read(addr, size),
            Some((cpu, offset)) => self.redist_read(cpu, offset, size),
        };
        Ok(value)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        match self.redist_of(addr) {
            None => self.dist_write(ctx, addr, value, size),
            Some((cpu, offset)) => self.redist_write(ctx, cpu, offset, value, size),
        }
        Ok(())
    }
}

impl EventTarget for GicV3 {
    fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
        Ok(())
    }

    fn on_signal(
        &mut self,
        ctx: &mut dyn Ctx,
        _source: mosaic_exec::NodeId,
        signal: SignalId,
        high: bool,
    ) -> Result<(), ContextError> {
        // The fabric-facing input: a device's IRQ output arrives as a
        // signal whose number is the INTID (SPIs).
        self.set_irq_level(ctx, signal.0, high);
        Ok(())
    }
}

impl Snapshot for GicV3 {
    type State = GicV3State;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state.clone()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.clone();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.u32(state.gicd_ctlr);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0xFF,
            endian: Endian::Little,
        }
    }

    struct RecCtx {
        signals: Vec<(SignalId, bool)>,
    }
    impl RecCtx {
        fn new() -> Self {
            Self {
                signals: Vec::new(),
            }
        }
        fn level(&self, id: SignalId) -> Option<bool> {
            self.signals
                .iter()
                .rev()
                .find(|(s, _)| *s == id)
                .map(|(_, high)| *high)
        }
    }
    impl Ctx for RecCtx {
        fn now(&self) -> mosaic_exec::VTime {
            mosaic_exec::VTime::ZERO
        }
        fn schedule(
            &mut self,
            _at: mosaic_exec::VTime,
            _token: u64,
        ) -> Result<mosaic_exec::EventId, ContextError> {
            Err(ContextError::ResourceExhausted)
        }
        fn cancel(&mut self, _event: mosaic_exec::EventId) -> bool {
            false
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    fn w(gic: &mut GicV3, ctx: &mut RecCtx, addr: u64, value: u64) {
        gic.write(ctx, addr, AccessSize::Word, value, &attr())
            .unwrap();
    }
    fn r(gic: &mut GicV3, ctx: &mut RecCtx, addr: u64) -> u64 {
        gic.read(ctx, addr, AccessSize::Word, &attr()).unwrap()
    }

    #[test]
    fn typer_reports_lines_and_cpu_count() {
        let mut gic = GicV3::new(GicV3Params::default());
        let mut ctx = RecCtx::new();
        let typer = r(&mut gic, &mut ctx, 0x0004) as u32;
        assert_eq!(typer & 0x1F, 7); // 256 INTIDs = 8 lines
        assert_eq!((typer >> 8) & 0xFF, 0); // one CPU
    }

    #[test]
    fn pidr2_advertises_gicv3() {
        let mut gic = GicV3::new(GicV3Params::default());
        let mut ctx = RecCtx::new();
        assert_eq!((r(&mut gic, &mut ctx, 0xFFE8) as u32 >> 4) & 0xF, 2);
        assert_eq!(
            (r(&mut gic, &mut ctx, GICD_FRAME + 0xFFE8) as u32 >> 4) & 0xF,
            2
        );
    }

    #[test]
    fn spi_enable_pending_and_acknowledge_cycle() {
        let mut gic = GicV3::new(GicV3Params::default());
        gic.bind_irq_output(0, SignalId(0));
        let mut ctx = RecCtx::new();
        w(
            &mut gic,
            &mut ctx,
            0x0000,
            u64::from(GICD_CTLR_DS | GICD_CTLR_EN_GRP1NS),
        );
        gic.state.grp1_enabled[0] = true;
        gic.state.pmr[0] = 0xFF;
        // Group 1 + enable INTID 33 (SPI 1): IGROUPR/ISENABLER word 1.
        w(&mut gic, &mut ctx, 0x0084, 1 << 1);
        w(&mut gic, &mut ctx, 0x0104, 1 << 1);
        gic.set_irq_level(&mut ctx, 33, true);
        assert_eq!(ctx.level(SignalId(0)), Some(true));
        assert_eq!(gic.acknowledge(&mut ctx, 0), 33);
        assert_eq!(ctx.level(SignalId(0)), Some(false));
        gic.end_of_interrupt(&mut ctx, 0, 33);
        assert_eq!(self_view(&gic, 33, |s| s.active.clone()), 0);
    }

    fn self_view(gic: &GicV3, intid: u32, f: fn(&GicV3State) -> Vec<u32>) -> u32 {
        let banks = f(&gic.state);
        let index = gic.word_index(0, intid).unwrap();
        (banks[index] >> (intid % 32)) & 1
    }

    #[test]
    fn group0_routes_to_fiq_line() {
        let mut gic = GicV3::new(GicV3Params::default());
        gic.bind_fiq_output(0, SignalId(7));
        let mut ctx = RecCtx::new();
        w(&mut gic, &mut ctx, 0x0000, u64::from(GICD_CTLR_EN_GRP0));
        gic.state.pmr[0] = 0xFF;
        w(&mut gic, &mut ctx, 0x0100, 1); // enable INTID 0 (SGI 0)
        gic.set_irq_level_for(&mut ctx, 0, 0, true);
        assert_eq!(ctx.level(SignalId(7)), Some(true));
    }

    #[test]
    fn setspi_nsr_generates_a_pending_spi() {
        let mut gic = GicV3::new(GicV3Params::default());
        let mut ctx = RecCtx::new();
        w(&mut gic, &mut ctx, 0x0104, 1 << 2); // enable INTID 34
        w(&mut gic, &mut ctx, 0x0040, 34);
        assert_ne!(r(&mut gic, &mut ctx, 0x0204) as u32 & (1 << 2), 0);
        w(&mut gic, &mut ctx, 0x0048, 34);
        assert_eq!(r(&mut gic, &mut ctx, 0x0204) as u32 & (1 << 2), 0);
    }

    #[test]
    fn irouter_routes_an_spi_to_its_cpu() {
        let mut gic = GicV3::new(GicV3Params { spis: 32, cpus: 2 });
        gic.bind_irq_output(1, SignalId(1));
        let mut ctx = RecCtx::new();
        w(
            &mut gic,
            &mut ctx,
            0x0000,
            u64::from(GICD_CTLR_DS | GICD_CTLR_EN_GRP1NS),
        );
        gic.state.grp1_enabled = vec![true, true];
        gic.state.pmr = vec![0xFF, 0xFF];
        w(&mut gic, &mut ctx, 0x0084, 1 << 3); // INTID 35 Group 1
        w(&mut gic, &mut ctx, 0x0104, 1 << 3); // INTID 35 enable
        gic.write(&mut ctx, 0x6000 + 3 * 8, AccessSize::DoubleWord, 1, &attr())
            .unwrap();
        gic.set_irq_level(&mut ctx, 35, true);
        assert_eq!(ctx.level(SignalId(1)), Some(true));
    }

    #[test]
    fn redistributor_frame_decodes_and_waker_sleeps() {
        let mut gic = GicV3::new(GicV3Params::default());
        gic.bind_irq_output(0, SignalId(0));
        let mut ctx = RecCtx::new();
        let rd = GICD_FRAME;
        let typer = gic
            .read(&mut ctx, rd + 0x0008, AccessSize::DoubleWord, &attr())
            .unwrap();
        assert_eq!(typer & GICR_TYPER_LAST, GICR_TYPER_LAST);
        // SGI_base: INTID 5 as Group 1 + enabled on cpu 0's bank.
        w(&mut gic, &mut ctx, rd + GICR_SGI_OFFSET + 0x0080, 1 << 5);
        w(&mut gic, &mut ctx, rd + GICR_SGI_OFFSET + 0x0100, 1 << 5);
        assert_eq!(
            r(&mut gic, &mut ctx, rd + GICR_SGI_OFFSET + 0x0100) as u32,
            1 << 5
        );
        // ProcessorSleep forces the outputs low.
        w(
            &mut gic,
            &mut ctx,
            0x0000,
            u64::from(GICD_CTLR_DS | GICD_CTLR_EN_GRP1NS),
        );
        gic.state.grp1_enabled[0] = true;
        gic.state.pmr[0] = 0xFF;
        gic.set_irq_level_for(&mut ctx, 0, 5, true);
        assert_eq!(ctx.level(SignalId(0)), Some(true));
        w(&mut gic, &mut ctx, rd + 0x0014, GICR_WAKER_PROCESSOR_SLEEP);
        assert_eq!(
            r(&mut gic, &mut ctx, rd + 0x0014)
                & (GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP),
            GICR_WAKER_PROCESSOR_SLEEP | GICR_WAKER_CHILDREN_ASLEEP
        );
        assert_eq!(ctx.level(SignalId(0)), Some(false));
    }

    #[test]
    fn sgi_via_gicd_sgir_pends_the_target_cpu() {
        let mut gic = GicV3::new(GicV3Params { spis: 32, cpus: 2 });
        let mut ctx = RecCtx::new();
        // GICD_SGIR: SGI 2, target list = cpu 1 (bit 1), filter 0.
        w(&mut gic, &mut ctx, 0x0F00, (2 << 16) | 2);
        let rd1 = GICD_FRAME + GICR_STRIDE;
        assert_ne!(
            r(&mut gic, &mut ctx, rd1 + GICR_SGI_OFFSET + 0x0200) as u32 & (1 << 2),
            0
        );
        assert_eq!(
            r(&mut gic, &mut ctx, GICD_FRAME + GICR_SGI_OFFSET + 0x0200) as u32 & (1 << 2),
            0
        );
    }

    #[test]
    fn itargetsr_is_read_as_zero() {
        let mut gic = GicV3::new(GicV3Params::default());
        let mut ctx = RecCtx::new();
        w(&mut gic, &mut ctx, 0x0800, 0xFF);
        assert_eq!(r(&mut gic, &mut ctx, 0x0800), 0);
    }

    #[test]
    fn priority_bytes_are_writable_per_interrupt() {
        let mut gic = GicV3::new(GicV3Params::default());
        let mut ctx = RecCtx::new();
        gic.write(&mut ctx, 0x0400 + 40, AccessSize::Byte, 0x80, &attr())
            .unwrap();
        assert_eq!(
            gic.read(&mut ctx, 0x0400 + 40, AccessSize::Byte, &attr())
                .unwrap(),
            0x80
        );
    }
}
