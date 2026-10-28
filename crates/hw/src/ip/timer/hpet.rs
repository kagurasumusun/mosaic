//! The IA-PC High Precision Event Timer (HPET) — QEMU's `hw/timer/hpet.c`
//! role.
//!
//! Clean-room from the IA-PC HPET Specification 1.0: the capability and ID
//! register (counter period in femtoseconds, timer count, legacy
//! replacement support), general configuration (ENABLE, LEGACY_ROUTE),
//! the 64-bit main counter that advances at the period, and three
//! comparators each with a configuration (level/edge, periodic, interrupt
//! enable), a 64-bit comparator value and an interrupt status bit. Timers
//! with `TN_INT_ENB` fire their output signal when the main counter passes
//! the comparator; periodic timers reload by the stored delta.
//!
//! Virtual time drives the counter: the model schedules a wake at each
//! armed comparator's deadline through [`Ctx::schedule`], so IRQ delivery
//! lands on the runtime's event queue, not on reads.

use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{
    AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError, VDelta, VTime,
};

/// The HPET's counter period in femtoseconds: 10 ns (the spec's example
/// value and what every Intel ICH reports at `0x989680`).
pub const COUNTER_PERIOD_FS: u64 = 10_000_000;
/// Femtoseconds per picosecond (VTime's unit).
const FS_PER_PS: u64 = 1_000;
/// The comparator count this model implements (the spec's minimum 3).
pub const TIMER_COUNT: usize = 3;

mod offset {
    pub const CAPABILITIES: u64 = 0x000;
    pub const GENERAL_CONFIG: u64 = 0x010;
    pub const GENERAL_INTERRUPT: u64 = 0x020;
    pub const MAIN_COUNTER: u64 = 0x0F0;
    pub const TIMER_CONFIG: u64 = 0x100;
    pub const TIMER_COMPARATOR: u64 = 0x108;
    pub const TIMER_FSB: u64 = 0x110;
    pub const TIMER_STRIDE: u64 = 0x20;
}

/// General config bits.
pub mod gen_config {
    pub const ENABLE: u64 = 1 << 0;
    pub const LEGACY_ROUTE: u64 = 1 << 1;
}

/// Timer config bits (the ones the register model decodes).
mod within {
    /// Offsets relative to a timer bank's base (TIMER_CONFIG).
    pub const CONFIG: u64 = 0x00;
    pub const COMPARATOR: u64 = 0x08;
    pub const FSB: u64 = 0x10;
}

pub mod timer_config {
    pub const LEVEL: u64 = 1 << 1;
    pub const INTERRUPT_ENABLE: u64 = 1 << 2;
    pub const PERIODIC: u64 = 1 << 3;
    pub const PERIODIC_CAPABLE: u64 = 1 << 4;
}

/// One comparator bank.
#[derive(Clone, Debug, Default)]
struct HpetTimer {
    config: u64,
    comparator: u64,
    /// The periodic delta as programmed (comparator at write time).
    period: u64,
    interrupt_status: bool,
    output: Option<SignalId>,
}

/// The HPET.
pub struct Hpet {
    timers: [HpetTimer; TIMER_COUNT],
    general_config: u64,
    main_counter: u64,
    /// The counter's virtual-time base: `now()` when the counter was last
    /// synchronized.
    base_time: VTime,
    enabled_time: Option<VTime>,
}

impl Default for Hpet {
    fn default() -> Self {
        Self::new()
    }
}

impl Hpet {
    pub fn new() -> Self {
        Self {
            timers: std::array::from_fn(|_| HpetTimer::default()),
            general_config: 0,
            main_counter: 0,
            base_time: VTime::ZERO,
            enabled_time: None,
        }
    }

    /// Wire one comparator's output signal (the machine's wiring).
    pub fn bind_timer_irq(&mut self, timer: usize, signal: SignalId) {
        if let Some(bank) = self.timers.get_mut(timer) {
            bank.output = Some(signal);
        }
    }

    fn enabled(&self) -> bool {
        self.general_config & gen_config::ENABLE != 0
    }

    /// The main counter's value at `now`: the base plus elapsed femtoseconds
    /// whenever the counter runs.
    fn counter_at(&self, now: VTime) -> u64 {
        if let Some(enabled) = self.enabled_time {
            if now >= enabled {
                let elapsed_fs = (now.0 - enabled.0) * FS_PER_PS;
                return self
                    .main_counter
                    .wrapping_add(elapsed_fs / COUNTER_PERIOD_FS);
            }
        }
        self.main_counter
    }

    fn sync_counter(&mut self, now: VTime) {
        self.main_counter = self.counter_at(now);
        self.enabled_time = if self.enabled() { Some(now) } else { None };
        self.base_time = now;
    }

    /// Schedule wakes for every armed comparator (call after any state
    /// change; tokens are the timer index + 1).
    fn arm_timers(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let now = ctx.now();
        let enabled = self.enabled();
        let counter = self.counter_at(now);
        let deadlines: Vec<Option<u64>> = self
            .timers
            .iter()
            .map(|timer| {
                if timer.config & timer_config::INTERRUPT_ENABLE == 0 || !enabled {
                    return None;
                }
                timer.comparator.checked_sub(counter)
            })
            .collect();
        for (index, deadline) in deadlines.into_iter().enumerate() {
            let Some(ticks) = deadline else {
                continue;
            };
            if ticks == 0 {
                // Deadline passed: fire immediately.
                self.fire(ctx, index)?;
                continue;
            }
            let delta_fs = ticks * COUNTER_PERIOD_FS;
            let delta_ps = delta_fs / FS_PER_PS;
            let _ = ctx.schedule_after(VDelta(delta_ps.max(1)), (index + 1) as u64);
        }
        Ok(())
    }

    fn fire(&mut self, ctx: &mut dyn Ctx, index: usize) -> Result<(), ContextError> {
        let now = ctx.now();
        let Some(timer) = self.timers.get_mut(index) else {
            return Ok(());
        };
        if timer.config & timer_config::INTERRUPT_ENABLE == 0 {
            return Ok(());
        }
        timer.interrupt_status = true;
        if timer.config & timer_config::PERIODIC != 0 && timer.period != 0 {
            timer.comparator = timer.comparator.wrapping_add(timer.period);
        }
        let level = timer.interrupt_status && timer.config & timer_config::INTERRUPT_ENABLE != 0;
        let output = timer.output;
        if let Some(signal) = output {
            set_optional_signal(ctx, signal, level)?;
        }
        let _ = now;
        Ok(())
    }

    #[allow(dead_code)] // the ICR path the status-clear sequence grows into
    fn clear_interrupt(&mut self, ctx: &mut dyn Ctx, index: usize) -> Result<(), ContextError> {
        if let Some(timer) = self.timers.get_mut(index) {
            timer.interrupt_status = false;
            if let Some(signal) = timer.output {
                set_optional_signal(ctx, signal, false)?;
            }
        }
        Ok(())
    }

    fn read_register(
        &mut self,
        now: VTime,
        offset: u64,
        size: AccessSize,
    ) -> Result<u64, BusFault> {
        if offset % 8 != 0 || size != AccessSize::DoubleWord {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            offset::CAPABILITIES => {
                // Period in fs, timer count, 64-bit counters, 3 timers, no
                // legacy route bits, vendor 0x8086.
                0x8282_0001
                    | (0x8086 << 16)
                    | ((TIMER_COUNT as u64 - 1) << 8)
                    | (COUNTER_PERIOD_FS << 32)
            }
            offset::GENERAL_CONFIG => self.general_config,
            offset::GENERAL_INTERRUPT => 0,
            offset::MAIN_COUNTER => self.counter_at(now),
            offset::TIMER_CONFIG | offset::TIMER_COMPARATOR | offset::TIMER_FSB => {
                let index = ((offset - offset::TIMER_CONFIG) / offset::TIMER_STRIDE) as usize;
                let bank = (offset - offset::TIMER_CONFIG) % offset::TIMER_STRIDE;
                match (index, bank) {
                    (index, within::CONFIG) if index < TIMER_COUNT => {
                        self.timers[index].config | timer_config::PERIODIC_CAPABLE
                    }
                    (index, within::COMPARATOR) if index < TIMER_COUNT => {
                        self.timers[index].comparator
                    }
                    (index, within::FSB) if index < TIMER_COUNT => 0,
                    _ => 0,
                }
            }
            _ => 0,
        })
    }

    fn write_register(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
    ) -> Result<(), BusFault> {
        if offset % 8 != 0 || size != AccessSize::DoubleWord {
            return Err(BusFault::InvalidSize);
        }
        match offset {
            offset::CAPABILITIES => {}
            offset::GENERAL_CONFIG => {
                let was_enabled = self.enabled();
                self.general_config = value & (gen_config::ENABLE | gen_config::LEGACY_ROUTE);
                let now = ctx.now();
                if !was_enabled && self.enabled() {
                    self.enabled_time = Some(now);
                    self.main_counter = self.counter_at(now);
                } else if was_enabled && !self.enabled() {
                    self.sync_counter(now);
                }
                self.arm_timers(ctx).ok();
            }
            offset::MAIN_COUNTER => {
                self.main_counter = value;
                self.enabled_time = if self.enabled() {
                    Some(ctx.now())
                } else {
                    None
                };
                self.arm_timers(ctx).ok();
            }
            offset::TIMER_CONFIG | offset::TIMER_COMPARATOR | offset::TIMER_FSB => {
                let index = ((offset - offset::TIMER_CONFIG) / offset::TIMER_STRIDE) as usize;
                let bank = (offset - offset::TIMER_CONFIG) % offset::TIMER_STRIDE;
                if index >= TIMER_COUNT {
                    return Ok(());
                }
                match bank {
                    within::CONFIG => {
                        self.timers[index].config = value
                            & (timer_config::INTERRUPT_ENABLE
                                | timer_config::PERIODIC
                                | timer_config::LEVEL);
                        self.arm_timers(ctx).ok();
                    }
                    within::COMPARATOR => {
                        self.timers[index].comparator = value;
                        self.timers[index].period = value;
                        self.arm_timers(ctx).ok();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl mosaic_contract::mmio::MmioTarget for Hpet {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        self.read_register(ctx.now(), offset, size)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        self.write_register(ctx, offset, size, value)
    }
}

impl EventTarget for Hpet {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        let index = token as usize;
        if (1..=TIMER_COUNT).contains(&index) {
            self.fire(ctx, index - 1)?;
        }
        Ok(())
    }
}

/// Checkpoint state (ADR 0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HpetCheckpoint {
    pub schema: u32,
    pub timers: [(u64, u64, u64, bool); TIMER_COUNT],
    pub general_config: u64,
    pub main_counter: u64,
    pub base_time: u64,
    pub enabled: bool,
}

impl Snapshot for Hpet {
    type State = HpetCheckpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            timers: self
                .timers
                .iter()
                .map(|t| (t.config, t.comparator, t.period, t.interrupt_status))
                .collect::<Vec<_>>()
                .try_into()
                .unwrap(),
            general_config: self.general_config,
            main_counter: self.main_counter,
            base_time: self.base_time.0,
            enabled: self.enabled(),
        }
    }

    fn validate(&self, version: u32, _state: &Self::State) -> Result<(), SnapshotError> {
        if version == Self::SCHEMA_VERSION {
            Ok(())
        } else {
            Err(SnapshotError::VersionMismatch)
        }
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        for (index, (config, comparator, period, status)) in state.timers.iter().enumerate() {
            self.timers[index].config = *config;
            self.timers[index].comparator = *comparator;
            self.timers[index].period = *period;
            self.timers[index].interrupt_status = *status;
        }
        self.general_config = state.general_config;
        self.main_counter = state.main_counter;
        self.base_time = VTime(state.base_time);
        self.enabled_time = if state.enabled {
            Some(VTime(state.base_time))
        } else {
            None
        };
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.general_config.to_le_bytes());
        hash.bytes(&state.main_counter.to_le_bytes());
        for (config, comparator, _, _) in state.timers {
            hash.bytes(&config.to_le_bytes());
            hash.bytes(&comparator.to_le_bytes());
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_contract::mmio::MmioTarget;
    use mosaic_contract::model::ContextError;
    use mosaic_exec::{AccessKind, Endian, EventId, NodeId, Security};

    struct TestCtx {
        now_ps: u64,
        scheduled: Vec<(VTime, u64)>,
        signals: Vec<(SignalId, bool)>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                now_ps: 0,
                scheduled: Vec::new(),
                signals: Vec::new(),
            }
        }
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            VTime(self.now_ps)
        }

        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            self.scheduled.push((at, token));
            Ok(EventId(self.scheduled.len() as u64))
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
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    #[test]
    fn capabilities_report_period_and_timer_count() {
        let mut hpet = Hpet::new();
        let mut ctx = TestCtx::new();
        let caps = hpet
            .read(
                &mut ctx,
                offset::CAPABILITIES,
                AccessSize::DoubleWord,
                &attr(),
            )
            .unwrap();
        assert_eq!((caps >> 32) & 0xFFFF_FFFF, COUNTER_PERIOD_FS);
        assert_eq!((caps >> 8) & 0x1F, TIMER_COUNT as u64 - 1);
    }

    #[test]
    fn the_counter_advances_with_virtual_time() {
        let mut hpet = Hpet::new();
        let mut ctx = TestCtx::new();
        hpet.write(
            &mut ctx,
            offset::GENERAL_CONFIG,
            AccessSize::DoubleWord,
            gen_config::ENABLE,
            &attr(),
        )
        .unwrap();
        ctx.now_ps = 10_000_000_000; // 10 ms in picoseconds
        let counter = hpet
            .read(
                &mut ctx,
                offset::MAIN_COUNTER,
                AccessSize::DoubleWord,
                &attr(),
            )
            .unwrap();
        // 10 ms at 10 ns per tick = 10^6 ticks.
        assert_eq!(counter, 1_000_000);
    }

    #[test]
    fn an_armed_comparator_fires_and_periodic_reloads() {
        let mut hpet = Hpet::new();
        let mut ctx = TestCtx::new();
        hpet.bind_timer_irq(0, SignalId(2));
        hpet.write(
            &mut ctx,
            offset::GENERAL_CONFIG,
            AccessSize::DoubleWord,
            gen_config::ENABLE,
            &attr(),
        )
        .unwrap();
        hpet.write(
            &mut ctx,
            offset::TIMER_COMPARATOR,
            AccessSize::DoubleWord,
            1000,
            &attr(),
        )
        .unwrap();
        hpet.write(
            &mut ctx,
            offset::TIMER_CONFIG,
            AccessSize::DoubleWord,
            timer_config::INTERRUPT_ENABLE | timer_config::PERIODIC,
            &attr(),
        )
        .unwrap();
        // The wake was scheduled; deliver it.
        assert!(!ctx.scheduled.is_empty() || hpet.timers[0].interrupt_status);
        let token = 1; // timer 0 + 1
        hpet.on_timer(&mut ctx, token).unwrap();
        assert!(hpet.timers[0].interrupt_status);
        assert!(ctx.signals.contains(&(SignalId(2), true)));
        // Periodic reload: comparator advanced by the period.
        assert_eq!(hpet.timers[0].comparator, 2000);
    }
}
