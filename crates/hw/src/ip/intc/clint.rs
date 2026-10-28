//! RISC-V Core-Local Interruptor (CLINT) — the SiFive-compatible
//! `mtime`/`mtimecmp`/`msip` block every `virt`-class board carries
//! (QEMU's `hw/intc/riscv_aclint.c` role, the classic map).
//!
//! Clean-room from the RISC-V privileged specification's machine timer
//! chapter plus the SiFive CLINT map. One node decodes one window:
//!
//! ```text
//! 0x0000  msip[hart]     — word per hart: bit 0 drives the machine
//!                          software-interrupt input (an IPI)
//! 0x4000  mtimecmp[hart] — 64-bit per hart; timer fires when
//!                          mtime >= mtimecmp
//! 0xBFF8  mtime          — one 64-bit free-running counter for the block
//! ```
//!
//! `mtime` is *derived*, never stored: reads compute it from the machine's
//! virtual time (`ctx.now()` in picoseconds divided by the configured tick
//! period), which keeps it monotonic across snapshots. Each hart's
//! machine-timer interrupt output is driven by an event reservation at the
//! deadline (`mtime == mtimecmp`), so the line rises exactly at the tick —
//! the model never polls. Writing `mtimecmp` re-arms or cancels the
//! reservation; writing `mtime` shifts nothing (the virtual clock owns the
//! counter) and is recorded as an offset so a loader that restores a
//! counter value does not lose it.
//!
//! The reset value of `mtimecmp` is "never" (`u64::MAX` minus a guard, so
//! `mtime >= mtimecmp` cannot race), matching the spec's silence with the
//! least surprising behaviour.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError, VTime};

/// `msip` register base.
pub const MSIP_BASE: u64 = 0x0000;
/// `mtimecmp` register base.
pub const MTIMECMP_BASE: u64 = 0x4000;
/// `mtime` register offset.
pub const MTIME_OFFSET: u64 = 0xBFF8;

/// Default tick period: 10 MHz (100 ns per tick), the frequency QEMU's
/// `virt` board gives the CLINT.
pub const DEFAULT_HZ: u64 = 10_000_000;

/// One hart's software-interrupt output.
pub const TOKEN_MSIP: u64 = 1;
/// Timer event identity base: `TIMER_TOKEN + hart`.
pub const TOKEN_TIMER: u64 = 0x100;

#[derive(Clone, Copy, Debug)]
pub struct ClintParams {
    /// Hart (timer) count.
    pub harts: u32,
    /// Counter frequency in Hz.
    pub hz: u64,
}

impl Default for ClintParams {
    fn default() -> Self {
        Self {
            harts: 1,
            hz: DEFAULT_HZ,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClintState {
    pub schema: u32,
    /// msip word per hart (bit 0 significant).
    pub msip: Vec<u32>,
    /// mtimecmp per hart; `u64::MAX` guards "never".
    pub mtimecmp: Vec<u64>,
    /// Offset applied to virtual-time-derived reads (a loader restoring
    /// `mtime` writes here).
    pub mtime_offset: u64,
    /// Snapshot of the counter at save time.
    pub mtime: u64,
}

pub struct Clint {
    state: ClintState,
    params: ClintParams,
    /// Software-interrupt output per hart.
    msip_outputs: Vec<Option<SignalId>>,
    /// Timer-interrupt output per hart.
    mtip_outputs: Vec<Option<SignalId>>,
    /// Pending timer event per hart (for cancel/reschedule).
    events: Vec<Option<mosaic_exec::EventId>>,
}

impl Clint {
    pub fn new(params: ClintParams) -> Self {
        let harts = params.harts.max(1);
        let params = ClintParams {
            harts,
            hz: params.hz.max(1),
        };
        Self {
            state: ClintState {
                schema: 1,
                msip: vec![0; harts as usize],
                // Reset: the timer does not fire (guarded MAX).
                mtimecmp: vec![u64::MAX - 1; harts as usize],
                mtime_offset: 0,
                mtime: 0,
            },
            params,
            msip_outputs: vec![None; harts as usize],
            mtip_outputs: vec![None; harts as usize],
            events: vec![None; harts as usize],
        }
    }

    /// Wire a hart's machine software-interrupt (IPI) output.
    pub fn bind_msip_output(&mut self, hart: usize, signal: SignalId) {
        if let Some(slot) = self.msip_outputs.get_mut(hart) {
            *slot = Some(signal);
        }
    }

    /// Wire a hart's machine timer-interrupt output.
    pub fn bind_mtip_output(&mut self, hart: usize, signal: SignalId) {
        if let Some(slot) = self.mtip_outputs.get_mut(hart) {
            *slot = Some(signal);
        }
    }

    fn harts(&self) -> usize {
        self.params.harts.max(1) as usize
    }

    /// The counter value now: virtual picoseconds since machine start
    /// (plus any loader offset), quantized to ticks.
    pub fn mtime_now(&self, ctx: &dyn Ctx) -> u64 {
        let ps = ctx.now().0.wrapping_add(self.state.mtime_offset);
        ps / (1_000_000_000_000 / self.params.hz.min(1_000_000_000_000))
    }

    /// (Re)arm or drop a hart's timer reservation for the current
    /// `mtimecmp`. Called from every path that can change the deadline or
    /// the clock.
    fn rearm(&mut self, ctx: &mut dyn Ctx, hart: usize) {
        let now = self.mtime_now(ctx);
        let deadline_ticks = self.state.mtimecmp[hart];
        if deadline_ticks.saturating_sub(now) > 1 && deadline_ticks != u64::MAX - 1 {
            // Ticks remaining → picoseconds until the deadline.
            let ticks = deadline_ticks - now;
            let ps_per_tick = 1_000_000_000_000 / self.params.hz;
            let at = VTime(
                ctx.now()
                    .0
                    .saturating_add(ticks.saturating_mul(ps_per_tick).saturating_sub(1)),
            );
            let token = TOKEN_TIMER + hart as u64;
            let new_event = ctx.schedule(at, token).ok();
            if let Some(event) = self.events[hart] {
                let _ = ctx.cancel(event);
            }
            self.events[hart] = new_event;
        } else if self.state.mtimecmp[hart] <= now {
            // Already past: the line rises now, no reservation needed.
            if let Some(signal) = self.mtip_outputs[hart] {
                let _ = ctx.set_signal(signal, true);
            }
            if let Some(event) = self.events[hart].take() {
                let _ = ctx.cancel(event);
            }
        } else if let Some(event) = self.events[hart].take() {
            let _ = ctx.cancel(event);
        }
    }

    fn update_msip(&self, ctx: &mut dyn Ctx, hart: usize) {
        if let Some(signal) = self.msip_outputs.get(hart).copied().flatten() {
            let high = self.state.msip.get(hart).copied().unwrap_or(0) & 1 != 0;
            let _ = ctx.set_signal(signal, high);
        }
    }
}

impl Default for Clint {
    fn default() -> Self {
        Self::new(ClintParams::default())
    }
}

impl MmioTarget for Clint {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if (MTIME_OFFSET..MTIME_OFFSET + 8).contains(&addr) {
            // 32-bit halves of mtime, per the spec's RV32 access pattern;
            // 64-bit reads return the whole counter.
            let value = self.mtime_now(ctx);
            let shift = ((addr - MTIME_OFFSET) * 8) % 64;
            let mask = if size == AccessSize::DoubleWord {
                u64::MAX
            } else {
                (1u64 << (size.bytes() as u64 * 8)) - 1
            };
            return Ok((value >> shift) & mask);
        }
        if addr >= MTIMECMP_BASE && addr < MTIMECMP_BASE + 8 * self.harts() as u64 {
            let hart = ((addr - MTIMECMP_BASE) / 8) as usize;
            let shift = ((addr - MTIMECMP_BASE) % 8) * 8;
            let mask = if size == AccessSize::DoubleWord {
                u64::MAX
            } else {
                (1u64 << (size.bytes() as u64 * 8)) - 1
            };
            let value = self
                .state
                .mtimecmp
                .get(hart)
                .copied()
                .unwrap_or(u64::MAX - 1);
            return Ok((value >> shift) & mask);
        }
        if addr < MSIP_BASE + 4 * self.harts() as u64 && addr % 4 == 0 {
            let hart = ((addr - MSIP_BASE) / 4) as usize;
            return Ok(u64::from(self.state.msip.get(hart).copied().unwrap_or(0)));
        }
        Ok(0)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if (MTIME_OFFSET..MTIME_OFFSET + 8).contains(&addr) {
            // A loader restoring the counter records the offset; the
            // virtual clock stays authoritative for progress.
            let shift = ((addr - MTIME_OFFSET) * 8) % 64;
            let width = size.bytes() as u64 * 8;
            let mask = if width == 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            let now = self.mtime_now(ctx);
            let cleared = now & !(mask << shift);
            self.state.mtime_offset = self
                .state
                .mtime_offset
                .wrapping_add(((value & mask) << shift).wrapping_sub(now & (mask << shift)));
            let _ = cleared;
            for hart in 0..self.harts() {
                self.rearm(ctx, hart);
            }
            return Ok(());
        }
        if addr >= MTIMECMP_BASE && addr < MTIMECMP_BASE + 8 * self.harts() as u64 {
            let hart = ((addr - MTIMECMP_BASE) / 8) as usize;
            let shift = ((addr - MTIMECMP_BASE) % 8) * 8;
            let width = size.bytes() as u64 * 8;
            let mask = if width == 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            if let Some(slot) = self.state.mtimecmp.get_mut(hart) {
                *slot = (*slot & !(mask << shift)) | ((value & mask) << shift);
            }
            self.rearm(ctx, hart);
            // Arming a future deadline drops a previously raised line.
            if self.state.mtimecmp.get(hart).copied().unwrap_or(0) > self.mtime_now(ctx) {
                if let Some(signal) = self.mtip_outputs.get(hart).copied().flatten() {
                    let _ = ctx.set_signal(signal, false);
                }
            }
            return Ok(());
        }
        if addr < MSIP_BASE + 4 * self.harts() as u64 && addr % 4 == 0 {
            let hart = ((addr - MSIP_BASE) / 4) as usize;
            if let Some(slot) = self.state.msip.get_mut(hart) {
                *slot = value as u32 & 1;
            }
            self.update_msip(ctx, hart);
            return Ok(());
        }
        Ok(())
    }
}

impl EventTarget for Clint {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token == TOKEN_MSIP {
            return Ok(());
        }
        let hart = (token - TOKEN_TIMER) as usize;
        if hart >= self.harts() {
            return Ok(());
        }
        self.events[hart] = None;
        let now = self.mtime_now(ctx);
        if self.state.mtimecmp.get(hart).copied().unwrap_or(u64::MAX) <= now {
            if let Some(signal) = self.mtip_outputs.get(hart).copied().flatten() {
                let _ = ctx.set_signal(signal, true);
            }
        } else {
            // mtimecmp moved since the reservation; re-arm.
            self.rearm(ctx, hart);
        }
        Ok(())
    }

    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        _source: mosaic_exec::NodeId,
        _signal: SignalId,
        _high: bool,
    ) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for Clint {
    type State = ClintState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        let mut state = self.state.clone();
        // `mtime` is a derived value; freeze it for the record.
        state.mtime = 0;
        state
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
        for word in &state.msip {
            hash.u32(*word);
        }
        for cmp in &state.mtimecmp {
            hash.u64(*cmp);
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_exec::{AccessKind, Endian, EventId, NodeId, Security};
    use std::cell::RefCell;

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

    /// A context with a movable clock and recorded signal edges.
    struct SimCtx {
        now: VTime,
        signals: Vec<(SignalId, bool)>,
        scheduled: Vec<(VTime, u64)>,
        cell: RefCell<Vec<EventId>>,
    }
    impl SimCtx {
        fn new() -> Self {
            Self {
                now: VTime::ZERO,
                signals: Vec::new(),
                scheduled: Vec::new(),
                cell: RefCell::new(Vec::new()),
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
    impl Ctx for SimCtx {
        fn now(&self) -> VTime {
            self.now
        }
        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            self.scheduled.push((at, token));
            let id = EventId(self.cell.borrow().len() as u64 + 1);
            self.cell.borrow_mut().push(id);
            Ok(id)
        }
        fn cancel(&mut self, _event: EventId) -> bool {
            true
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.signals.push((signal, high));
            Ok(())
        }
    }

    #[test]
    fn msip_drives_the_software_interrupt_line() {
        let mut clint = Clint::new(ClintParams::default());
        clint.bind_msip_output(0, SignalId(2));
        let mut ctx = SimCtx::new();
        clint
            .write(&mut ctx, MSIP_BASE, AccessSize::Word, 1, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(2)), Some(true));
        clint
            .write(&mut ctx, MSIP_BASE, AccessSize::Word, 0, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(2)), Some(false));
    }

    #[test]
    fn mtime_advances_with_virtual_time() {
        let mut clint = Clint::new(ClintParams {
            harts: 1,
            hz: 1_000_000, // 1 µs tick
        });
        let mut ctx = SimCtx::new();
        ctx.now = VTime(3_500_000); // 3.5 µs → 3 ticks at 1 MHz
        let value = clint
            .read(&mut ctx, MTIME_OFFSET, AccessSize::DoubleWord, &attr())
            .unwrap();
        assert_eq!(value, 3);
        // RV32 halfword pattern: low word at +0, high at +4.
        let low = clint
            .read(&mut ctx, MTIME_OFFSET, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(low, 3);
        ctx.now = VTime(1_400_000_000_000); // 1.4e6 ticks
        let high = clint
            .read(&mut ctx, MTIME_OFFSET + 4, AccessSize::Word, &attr())
            .unwrap();
        assert_eq!(high, 1_400_000u64 >> 32); // high word of 1.4e6
    }

    #[test]
    fn mtimecmp_arms_and_fires() {
        let mut clint = Clint::new(ClintParams {
            harts: 1,
            hz: 1_000_000,
        });
        clint.bind_mtip_output(0, SignalId(5));
        let mut ctx = SimCtx::new();
        // At t=0, arm for tick 5 (5 µs out).
        clint
            .write(&mut ctx, MTIMECMP_BASE, AccessSize::DoubleWord, 5, &attr())
            .unwrap();
        assert_eq!(ctx.scheduled.len(), 1);
        assert_eq!(ctx.scheduled[0].1, TOKEN_TIMER);
        // Fire the reservation.
        ctx.now = VTime(5_000_000);
        clint.on_timer(&mut ctx, TOKEN_TIMER).unwrap();
        assert_eq!(ctx.level(SignalId(5)), Some(true));
        // A new, later deadline drops the line and re-arms.
        clint
            .write(&mut ctx, MTIMECMP_BASE, AccessSize::DoubleWord, 10, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(5)), Some(false));
        assert_eq!(ctx.scheduled.len(), 2);
    }

    #[test]
    fn past_deadline_raises_immediately() {
        let mut clint = Clint::new(ClintParams {
            harts: 1,
            hz: 1_000_000,
        });
        clint.bind_mtip_output(0, SignalId(1));
        let mut ctx = SimCtx::new();
        ctx.now = VTime(10_000_000); // tick 10
        clint
            .write(&mut ctx, MTIMECMP_BASE, AccessSize::DoubleWord, 3, &attr())
            .unwrap();
        assert_eq!(ctx.level(SignalId(1)), Some(true));
    }

    #[test]
    fn rv32_halves_of_mtimecmp() {
        let mut clint = Clint::new(ClintParams::default());
        let mut ctx = SimCtx::new();
        clint
            .write(
                &mut ctx,
                MTIMECMP_BASE,
                AccessSize::Word,
                0xFFFF_FFFF,
                &attr(),
            )
            .unwrap();
        clint
            .write(&mut ctx, MTIMECMP_BASE + 4, AccessSize::Word, 1, &attr())
            .unwrap();
        let value = clint
            .read(&mut ctx, MTIMECMP_BASE, AccessSize::DoubleWord, &attr())
            .unwrap();
        assert_eq!(value, 0x1_FFFF_FFFF);
    }

    #[test]
    fn multi_hart_frames_are_independent() {
        let mut clint = Clint::new(ClintParams {
            harts: 2,
            hz: 1_000_000,
        });
        clint.bind_mtip_output(1, SignalId(9));
        let mut ctx = SimCtx::new();
        clint
            .write(
                &mut ctx,
                MTIMECMP_BASE + 8,
                AccessSize::DoubleWord,
                4,
                &attr(),
            )
            .unwrap();
        // Hart 1's deadline reservation carries its own token.
        assert_eq!(ctx.scheduled[0].1, TOKEN_TIMER + 1);
        ctx.now = VTime(4_000_000);
        clint.on_timer(&mut ctx, TOKEN_TIMER + 1).unwrap();
        assert_eq!(ctx.level(SignalId(9)), Some(true));
    }

    #[test]
    fn mtimecmp_reset_never_fires() {
        let clint = Clint::new(ClintParams::default());
        assert_eq!(clint.state.mtimecmp[0], u64::MAX - 1);
    }
}
