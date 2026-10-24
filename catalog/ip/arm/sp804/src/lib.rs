//! ARM Dual-Timer Module (SP804) functional model.
//!
//! Clean-room behavior from ARM DDI 0271: two independent timers with load,
//! background load, control, raw/masked interrupt status and interrupt-clear
//! registers, plus the PrimeCell identification window. The model is purely
//! event driven: enabling a timer reserves one [`Ctx`] callback per expiry
//! instead of ticking every clock cycle, so virtual time stays the only clock
//! source and deterministic replay needs no extra inputs.
#![no_std]
#![forbid(unsafe_code)]

use core::num::NonZeroU64;

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_core_types::{
    AccessSize, Attr, BusFault, EventId, Fnv64, SignalId, Snapshot, SnapshotError, VDelta, VTime,
};
use mosaic_ip_arm_primecell_id::PrimeCellId;

/// Register offsets inside one timer block (DDI 0271 Table 3-2).
mod reg {
    pub const LOAD: u64 = 0x00;
    pub const VALUE: u64 = 0x04;
    pub const CONTROL: u64 = 0x08;
    pub const INT_CLR: u64 = 0x0C;
    pub const RIS: u64 = 0x10;
    pub const MIS: u64 = 0x14;
    pub const BG_LOAD: u64 = 0x18;
    /// Base offset of the second timer block.
    pub const TIMER2_BASE: u64 = 0x20;
}

/// TimerXControl bit positions (DDI 0271 Table 3-6).
mod ctrl {
    pub const ONESHOT: u32 = 1 << 0;
    pub const SIZE32: u32 = 1 << 1;
    pub const IE: u32 = 1 << 5;
    pub const PERIODIC: u32 = 1 << 6;
    pub const ENABLE: u32 = 1 << 7;
    /// Defined control bits: oneshot, size, prescale [3:2], IE, mode, enable.
    /// Bit 4 is reserved; the prescale field must survive the write mask
    /// (DDI 0271 Table 3-6).
    pub const MASK: u32 = 0xEF;
}

/// Timer tokens carried by scheduled callbacks; one per hardware timer.
const TOKEN_TIMER1: u64 = 0;
const TOKEN_TIMER2: u64 = 1;

/// Policy for offsets without a modeled register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored, the default for reserved space.
    RazWi,
    /// Report a guest-visible device fault.
    Fault,
}

/// Construction parameters. `tick_ps` is the calibrated input clock period in
/// picoseconds; the prescale field divides it inside each timer.
#[derive(Clone, Copy, Debug)]
pub struct Sp804Params {
    pub irq: [SignalId; 2],
    pub tick_ps: NonZeroU64,
    pub unmapped: UnmappedPolicy,
}

impl Sp804Params {
    pub const fn new(irq: [SignalId; 2], tick_ps: NonZeroU64) -> Self {
        Self {
            irq,
            tick_ps,
            unmapped: UnmappedPolicy::RazWi,
        }
    }
}

/// Snapshot-relevant state of one timer, gathered per design §10.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sp804TimerState {
    pub schema: u32,
    pub load: u32,
    pub bg_load: u32,
    pub control: u32,
    pub raw_irq: bool,
    pub enabled: bool,
    pub remaining: u32,
    pub started_at: VTime,
}

impl Default for Sp804TimerState {
    fn default() -> Self {
        Self {
            schema: 1,
            load: 0,
            bg_load: 0,
            control: 0,
            raw_irq: false,
            enabled: false,
            remaining: 0,
            started_at: VTime::ZERO,
        }
    }
}

/// One hardware timer. The scheduled event handle is runtime state and stays
/// outside the snapshot type.
#[derive(Default)]
struct Timer {
    state: Sp804TimerState,
    irq_asserted: bool,
    event: Option<EventId>,
}

impl Timer {
    fn prescale_divider(control: u32) -> u64 {
        match (control >> 2) & 0b11 {
            0 => 1,
            1 => 16,
            _ => 256,
        }
    }

    /// Mask a register value to the currently selected counter width.
    fn counter_mask(&self) -> u32 {
        if self.state.control & ctrl::SIZE32 != 0 {
            u32::MAX
        } else {
            u32::from(u16::MAX)
        }
    }

    fn counter_value(&self, value: u32) -> u32 {
        value & self.counter_mask()
    }

    /// Value loaded when the counter reaches zero: free-running wraps at the
    /// programmed width, periodic/one-shot use BGLoad (when set) else Load.
    fn reload_value(&self) -> u32 {
        let c = self.state.control;
        let value = if c & ctrl::PERIODIC == 0 && c & ctrl::ONESHOT == 0 {
            self.counter_mask()
        } else if self.state.bg_load != 0 {
            self.state.bg_load
        } else {
            self.state.load
        };
        // LOAD/BGLOAD remain 32-bit guest-visible registers in 16-bit mode;
        // only the running counter is truncated.
        self.counter_value(value)
    }

    /// Picoseconds between enable and the next expiry. Counts reload..0, so
    /// the span is reload + 1 ticks.
    fn delay_for_count_ps(&self, count: u32, tick_ps: NonZeroU64) -> Option<u64> {
        // The counter expires after count + 1 ticks, including one tick when
        // the programmed count is zero. Widen before multiplication so a
        // 32-bit reload cannot overflow an intermediate integer.
        let ticks = u64::from(count).checked_add(1)?;
        let period = tick_ps
            .get()
            .checked_mul(Self::prescale_divider(self.state.control))?;
        u128::from(ticks)
            .checked_mul(u128::from(period))
            .and_then(|delay| u64::try_from(delay).ok())
    }

    fn start_from(
        &mut self,
        ctx: &mut dyn Ctx,
        token: u64,
        tick_ps: NonZeroU64,
        initial_count: u32,
    ) -> Result<(), ContextError> {
        let now = ctx.now();
        let delay = self
            .delay_for_count_ps(initial_count, tick_ps)
            .ok_or(ContextError::TimeOverflow)?;
        let at = now
            .checked_add(VDelta(delay))
            .ok_or(ContextError::TimeOverflow)?;
        let event = match self.event {
            Some(previous) => ctx.reschedule(previous, at, token)?,
            None => ctx.schedule(at, token)?,
        };
        // Commit timer state only after its replacement event has been
        // accepted. A full queue or ID exhaustion must not silently orphan
        // the currently armed timer.
        self.event = Some(event);
        self.state.started_at = now;
        self.state.remaining = initial_count;
        self.state.enabled = true;
        Ok(())
    }

    fn start(
        &mut self,
        ctx: &mut dyn Ctx,
        token: u64,
        tick_ps: NonZeroU64,
    ) -> Result<(), ContextError> {
        self.start_from(ctx, token, tick_ps, self.reload_value())
    }

    #[cfg(test)]
    fn delay_ps(&self, tick_ps: NonZeroU64) -> Option<u64> {
        self.delay_for_count_ps(self.reload_value(), tick_ps)
    }

    /// Cancel the pending expiry and freeze the counter at `frozen`, the
    /// value computed with the configuration that was active while running.
    fn stop(&mut self, ctx: &mut dyn Ctx, frozen: u32) {
        if let Some(event) = self.event.take() {
            let _ = ctx.cancel(event);
        }
        self.state.remaining = frozen;
        self.state.enabled = false;
    }

    /// Remaining count derived purely from elapsed virtual time, so reads
    /// between scheduled expiries need no per-cycle bookkeeping.
    fn current_value(&self, now: VTime, tick_ps: NonZeroU64) -> u32 {
        if !self.state.enabled {
            return self.state.remaining;
        }
        let period = tick_ps
            .get()
            .saturating_mul(Self::prescale_divider(self.state.control));
        let elapsed_ticks = now.0.saturating_sub(self.state.started_at.0) / period;
        let start_count = self.state.remaining;
        // The active reservation expires after start_count + 1 ticks. A
        // control write can re-arm from a frozen live count, which may differ
        // from LOAD/BGLOAD; deriving VALUE from reload_value would jump the
        // counter backwards after such a write.
        let span = u64::from(start_count) + 1;
        let into_span = elapsed_ticks % span;
        start_count.saturating_sub(u32::try_from(into_span).unwrap_or(0))
    }
}

/// Functional SP804 with both timer blocks.
pub struct Sp804 {
    params: Sp804Params,
    timers: [Timer; 2],
    identity: PrimeCellId,
}

impl Sp804 {
    /// SP804 part number 0x084, first revision, no patch.
    pub const PART_NUMBER: u16 = 0x084;

    pub fn new(params: Sp804Params) -> Self {
        Self {
            params,
            timers: [Timer::default(), Timer::default()],
            identity: PrimeCellId::arm(Self::PART_NUMBER, 0, 0)
                .expect("fixed SP804 identity fields are valid"),
        }
    }

    /// Re-evaluate one timer's interrupt line. An unwired pin is tolerated:
    /// the context reports it unknown and the timer keeps running.
    fn drive_irq(ctx: &mut dyn Ctx, timer: &mut Timer, irq: SignalId) -> Result<(), ContextError> {
        let asserted = timer.state.raw_irq && timer.state.control & ctrl::IE != 0;
        if asserted != timer.irq_asserted {
            set_optional_signal(ctx, irq, asserted)?;
            timer.irq_asserted = asserted;
        }
        Ok(())
    }

    fn timer_index(offset: u64) -> (usize, u64) {
        if offset >= reg::TIMER2_BASE {
            (1, offset - reg::TIMER2_BASE)
        } else {
            (0, offset)
        }
    }

    fn read_word(&mut self, ctx: &mut dyn Ctx, offset: u64) -> Result<u32, BusFault> {
        if offset % 4 != 0 {
            return self.unmapped();
        }
        if PrimeCellId::covers(offset) {
            return Ok(self.identity.read_word(offset).unwrap_or(0));
        }
        let (index, local) = Self::timer_index(offset);
        if local > reg::BG_LOAD {
            return self.unmapped();
        }
        let timer = &mut self.timers[index];
        let value = match local {
            reg::LOAD => timer.state.load,
            reg::VALUE => timer.current_value(ctx.now(), self.params.tick_ps),
            reg::CONTROL => timer.state.control,
            reg::INT_CLR => 0,
            reg::RIS => u32::from(timer.state.raw_irq),
            reg::MIS => u32::from(timer.state.raw_irq && timer.state.control & ctrl::IE != 0),
            reg::BG_LOAD => timer.state.bg_load,
            _ => return self.unmapped(),
        };
        Ok(value)
    }

    fn write_word(&mut self, ctx: &mut dyn Ctx, offset: u64, value: u32) -> Result<(), BusFault> {
        if offset % 4 != 0 {
            return self.unmapped().map(|_| ());
        }
        if PrimeCellId::covers(offset) {
            // Identification registers are read-only and ignore writes.
            return Ok(());
        }
        let (index, local) = Self::timer_index(offset);
        if local > reg::BG_LOAD {
            return self.unmapped().map(|_| ());
        }
        let token = if index == 0 {
            TOKEN_TIMER1
        } else {
            TOKEN_TIMER2
        };
        let tick_ps = self.params.tick_ps;
        let irq = self.params.irq[index];
        let timer = &mut self.timers[index];
        match local {
            reg::LOAD => {
                let previous_load = timer.state.load;
                timer.state.load = value;
                if timer.state.enabled {
                    // Hardware restarts immediately when Load is written live.
                    if timer
                        .start_from(ctx, token, tick_ps, timer.counter_value(value))
                        .is_err()
                    {
                        timer.state.load = previous_load;
                        return Err(BusFault::Device);
                    }
                } else {
                    timer.state.remaining = timer.counter_value(value);
                }
            }
            reg::BG_LOAD => timer.state.bg_load = value,
            reg::CONTROL => {
                let previous_control = timer.state.control;
                let was_enabled = timer.state.enabled;
                // Freeze the counter with the OLD configuration first: a
                // control write must not reinterpret already elapsed time
                // through the new mode/width (DDI 0271).
                let frozen = timer.current_value(ctx.now(), tick_ps);
                timer.state.control = value & ctrl::MASK;
                let now_enabled = timer.state.control & ctrl::ENABLE != 0;
                match (was_enabled, now_enabled) {
                    (false, true) => {
                        if timer.start(ctx, token, tick_ps).is_err() {
                            timer.state.control = previous_control;
                            return Err(BusFault::Device);
                        }
                    }
                    (true, true) => {
                        // Preserve the count frozen with the old control
                        // configuration. Re-arming with the reload value here
                        // would reset the counter on every prescale/IE/mode
                        // update, rather than merely applying the new timing.
                        if timer.start_from(ctx, token, tick_ps, frozen).is_err() {
                            timer.state.control = previous_control;
                            return Err(BusFault::Device);
                        }
                    }
                    (true, false) => timer.stop(ctx, frozen),
                    (false, false) => {}
                }
            }
            reg::INT_CLR => {
                timer.state.raw_irq = false;
            }
            reg::VALUE | reg::RIS | reg::MIS => {}
            _ => return self.unmapped().map(|_| ()),
        }
        Self::drive_irq(ctx, timer, irq).map_err(|_| BusFault::Device)?;
        Ok(())
    }

    fn unmapped(&self) -> Result<u32, BusFault> {
        match self.params.unmapped {
            UnmappedPolicy::RazWi => Ok(0),
            UnmappedPolicy::Fault => Err(BusFault::Device),
        }
    }
}

impl MmioTarget for Sp804 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if matches!(size, AccessSize::DoubleWord) {
            // A 64-bit access is not a supported SP804 register transaction.
            return Err(BusFault::InvalidSize);
        }
        let width = u32::from(size.bytes()) * 8;
        let mask = if width >= 32 {
            u32::MAX
        } else {
            (1u32 << width) - 1
        };
        let word = self.read_word(ctx, offset)?;
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
            // SP804 registers are 32-bit; an 8-byte transfer cannot fit.
            return Err(BusFault::InvalidSize);
        }
        self.write_word(ctx, offset, value as u32)
    }
}

impl EventTarget for Sp804 {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        // Tokens are issued by this model's two start paths. A stale or
        // corrupt token must not be clamped onto timer 2, which could assert
        // an unrelated IRQ and mutate the wrong counter.
        let index = match token {
            TOKEN_TIMER1 => 0,
            TOKEN_TIMER2 => 1,
            _ => return Ok(()),
        };
        let irq = self.params.irq[index];
        let tick_ps = self.params.tick_ps;
        let timer = &mut self.timers[index];
        if !timer.state.enabled {
            // A stale callback after disable must not touch device state.
            return Ok(());
        }
        timer.event = None;
        timer.state.raw_irq = true;
        if timer.state.control & ctrl::ONESHOT != 0 {
            timer.state.control &= !ctrl::ENABLE;
            timer.state.enabled = false;
            timer.state.remaining = 0;
            Self::drive_irq(ctx, timer, irq)?;
            return Ok(());
        }
        // Periodic and free-running timers re-arm from the reload value.
        // If the queue cannot accept the next expiry, fail closed: clear the
        // enable state, but still attempt to deliver the expiry interrupt.
        let rearm = timer.start(ctx, token, tick_ps);
        if rearm.is_err() {
            timer.state.control &= !ctrl::ENABLE;
            timer.state.enabled = false;
            timer.event = None;
        }
        let irq_result = Self::drive_irq(ctx, timer, irq);
        rearm.and(irq_result)
    }
}

/// Both timer blocks, including the runtime event handle. The queue restores
/// the same [`EventId`], so the timer must remember which reservation it owns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sp804Checkpoint {
    pub schema: u32,
    pub timers: [Sp804TimerState; 2],
    pub irq_asserted: [bool; 2],
    pub events: [Option<EventId>; 2],
}

impl Snapshot for Sp804 {
    type State = Sp804Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Sp804Checkpoint {
            schema: Self::SCHEMA_VERSION,
            timers: [self.timers[0].state, self.timers[1].state],
            irq_asserted: [self.timers[0].irq_asserted, self.timers[1].irq_asserted],
            events: [self.timers[0].event, self.timers[1].event],
        }
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        if state.timers.iter().any(|timer| timer.schema != 1) {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        for (timer, index) in self.timers.iter_mut().zip(0..2) {
            timer.state = state.timers[index];
            timer.irq_asserted = state.irq_asserted[index];
            timer.event = state.events[index];
        }
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::new();
        hash.u32(version);
        for (timer, asserted, event) in state
            .timers
            .iter()
            .zip(state.irq_asserted)
            .zip(state.events)
            .map(|((timer, asserted), event)| (timer, asserted, event))
        {
            hash.u32(timer.load);
            hash.u32(timer.bg_load);
            hash.u32(timer.control);
            hash.bool(timer.raw_irq);
            hash.bool(timer.enabled);
            hash.u32(timer.remaining);
            hash.u64(timer.started_at.0);
            hash.bool(asserted);
            hash.option_u64(event.map(|id| id.0));
        }
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::prelude::v1::*;

    use super::*;
    use std::collections::VecDeque;
    use std::vec;

    const IRQ1: SignalId = SignalId(2);
    const IRQ2: SignalId = SignalId(3);
    const TICK: NonZeroU64 = match NonZeroU64::new(1_000) {
        Some(v) => v,
        None => unreachable!(),
    };

    /// Context with a real timer queue and a record of signal edges.
    struct TestCtx {
        now: VTime,
        next_id: u64,
        queue: VecDeque<(VTime, u64, EventId)>,
        edges: Vec<(SignalId, bool)>,
    }

    impl Default for TestCtx {
        fn default() -> Self {
            Self::new()
        }
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                now: VTime::ZERO,
                next_id: 1,
                queue: VecDeque::new(),
                edges: Vec::new(),
            }
        }

        fn advance(&mut self, ps: u64) {
            self.now = VTime(self.now.0 + ps);
        }

        fn pop_due(&mut self) -> Option<(VTime, u64)> {
            self.queue
                .iter()
                .position(|(at, _, _)| *at <= self.now)
                .map(|pos| {
                    let (at, token, _) = self.queue.remove(pos).unwrap();
                    (at, token)
                })
        }
    }

    impl Ctx for TestCtx {
        fn now(&self) -> VTime {
            self.now
        }
        fn schedule(&mut self, at: VTime, token: u64) -> Result<EventId, ContextError> {
            let id = EventId(self.next_id);
            self.next_id += 1;
            self.queue.push_back((at, token, id));
            Ok(id)
        }
        fn cancel(&mut self, event: EventId) -> bool {
            if let Some(pos) = self.queue.iter().position(|(_, _, id)| *id == event) {
                self.queue.remove(pos);
                true
            } else {
                false
            }
        }
        fn set_signal(&mut self, signal: SignalId, high: bool) -> Result<(), ContextError> {
            self.edges.push((signal, high));
            Ok(())
        }
    }

    fn params() -> Sp804Params {
        Sp804Params::new([IRQ1, IRQ2], TICK)
    }

    fn word_attr() -> Attr {
        Attr {
            master: mosaic_core_types::NodeId(0),
            security: mosaic_core_types::Security::NonSecure,
            privileged: false,
            kind: mosaic_core_types::AccessKind::Data,
            exclusive: false,
            byte_enable: u8::MAX,
            endian: mosaic_core_types::Endian::Little,
        }
    }

    #[test]
    fn periodic_expiry_fires_irq_and_reschedules() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804.write_word(&mut ctx, reg::LOAD, 9).unwrap();
        // Periodic + interrupt enable + 32-bit + enable.
        sp804
            .write_word(
                &mut ctx,
                reg::CONTROL,
                ctrl::PERIODIC | ctrl::IE | ctrl::SIZE32 | ctrl::ENABLE,
            )
            .unwrap();
        // Ten ticks (load 9 counts 9..0) at 1000 ps each.
        assert_eq!(sp804.timers[0].delay_ps(TICK), Some(10_000));
        ctx.advance(5_000);
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), 4);
        assert!(ctx.pop_due().is_none());
        ctx.advance(5_000);
        let (_, token) = ctx.pop_due().expect("expiry is due at 10 ns");
        sp804.on_timer(&mut ctx, token).unwrap();
        assert_eq!(sp804.read_word(&mut ctx, reg::RIS).unwrap(), 1);
        assert_eq!(sp804.read_word(&mut ctx, reg::MIS).unwrap(), 1);
        assert_eq!(ctx.edges, vec![(IRQ1, true)]);
        // A fresh periodic reservation replaced the consumed one.
        assert!(sp804.timers[0].event.is_some());
        sp804.write_word(&mut ctx, reg::INT_CLR, 0).unwrap();
        assert_eq!(sp804.read_word(&mut ctx, reg::MIS).unwrap(), 0);
        assert_eq!(ctx.edges.last(), Some(&(IRQ1, false)));
    }

    #[test]
    fn one_shot_expiry_disables_the_timer_without_reschedule() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804.write_word(&mut ctx, reg::LOAD, 4).unwrap();
        sp804
            .write_word(
                &mut ctx,
                reg::CONTROL,
                ctrl::ONESHOT | ctrl::IE | ctrl::ENABLE,
            )
            .unwrap();
        ctx.advance(5_000);
        let (_, token) = ctx.pop_due().unwrap();
        sp804.on_timer(&mut ctx, token).unwrap();
        assert!(!sp804.timers[0].state.enabled);
        assert!(sp804.timers[0].event.is_none());
        assert_eq!(sp804.timers[0].state.control & ctrl::ENABLE, 0);
        assert_eq!(ctx.edges, vec![(IRQ1, true)]);
    }

    #[test]
    fn disabled_timer_freezes_value_and_cancels_its_event() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804.write_word(&mut ctx, reg::LOAD, 100).unwrap();
        let control = ctrl::PERIODIC | ctrl::SIZE32 | ctrl::ENABLE;
        sp804.write_word(&mut ctx, reg::CONTROL, control).unwrap();
        assert!(sp804.timers[0].event.is_some());
        ctx.advance(10_000);
        // Disable with IE clear: no interrupt edges occur during the test.
        sp804.write_word(&mut ctx, reg::CONTROL, 0).unwrap();
        assert!(sp804.timers[0].event.is_none());
        let frozen = sp804.read_word(&mut ctx, reg::VALUE).unwrap();
        assert_eq!(frozen, 90);
        ctx.advance(50_000);
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), frozen);
        assert!(ctx.edges.is_empty());
    }

    #[test]
    fn sixteen_bit_mode_truncates_count_without_truncating_load_register() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804.write_word(&mut ctx, reg::LOAD, 0x1_0005).unwrap();
        sp804
            .write_word(&mut ctx, reg::CONTROL, ctrl::ONESHOT | ctrl::ENABLE)
            .unwrap();

        // LOAD remains a 32-bit register, but the selected 16-bit timer counts
        // from 5 and expires after six ticks rather than 0x10006 ticks.
        assert_eq!(sp804.read_word(&mut ctx, reg::LOAD).unwrap(), 0x1_0005);
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), 5);
        assert_eq!(sp804.timers[0].delay_ps(TICK), Some(6_000));
    }

    #[test]
    fn live_control_update_preserves_the_frozen_counter_value() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804.write_word(&mut ctx, reg::LOAD, 100).unwrap();
        sp804
            .write_word(
                &mut ctx,
                reg::CONTROL,
                ctrl::PERIODIC | ctrl::SIZE32 | ctrl::ENABLE,
            )
            .unwrap();

        ctx.advance(10_000);
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), 90);

        // Enabling IRQ while keeping the timer live must not reload LOAD=100.
        sp804
            .write_word(
                &mut ctx,
                reg::CONTROL,
                ctrl::PERIODIC | ctrl::SIZE32 | ctrl::ENABLE | ctrl::IE,
            )
            .unwrap();
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), 90);
        ctx.advance(5_000);
        assert_eq!(sp804.read_word(&mut ctx, reg::VALUE).unwrap(), 85);
    }

    #[test]
    fn timer2_block_and_prescale_are_independent() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        let local_control = reg::TIMER2_BASE + reg::CONTROL;
        let local_load = reg::TIMER2_BASE + reg::LOAD;
        sp804.write_word(&mut ctx, local_load, 9).unwrap();
        // Prescale /16 plus IE and enable on timer 2 only.
        sp804
            .write_word(
                &mut ctx,
                local_control,
                (1 << 2) | ctrl::IE | ctrl::PERIODIC | ctrl::ENABLE,
            )
            .unwrap();
        assert_eq!(sp804.timers[1].delay_ps(TICK), Some(160_000));
        assert!(sp804.timers[0].event.is_none());
        ctx.advance(160_000);
        let (_, token) = ctx.pop_due().unwrap();
        assert_eq!(token, TOKEN_TIMER2);
        sp804.on_timer(&mut ctx, token).unwrap();
        assert_eq!(ctx.edges, vec![(IRQ2, true)]);
    }

    #[test]
    fn invalid_timer_token_does_not_alias_timer2() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        sp804
            .write_word(&mut ctx, reg::TIMER2_BASE + reg::LOAD, 1)
            .unwrap();
        sp804
            .write_word(
                &mut ctx,
                reg::TIMER2_BASE + reg::CONTROL,
                ctrl::ONESHOT | ctrl::IE | ctrl::ENABLE,
            )
            .unwrap();
        let pending = sp804.timers[1].event;
        let before = sp804.timers[1].state;
        sp804.on_timer(&mut ctx, u64::MAX).unwrap();
        assert_eq!(sp804.timers[1].event, pending);
        assert_eq!(sp804.timers[1].state, before);
        assert!(ctx.edges.is_empty());
    }

    #[test]
    fn primecell_window_and_guest_faults_are_safe() {
        let mut sp804 = Sp804::new(params());
        let mut ctx = TestCtx::new();
        assert_eq!(
            sp804
                .read(&mut ctx, 0xFE0, AccessSize::Word, &word_attr())
                .unwrap(),
            0x84
        );
        assert_eq!(
            sp804
                .read(&mut ctx, 0x900, AccessSize::Word, &word_attr())
                .unwrap(),
            0
        );
        let mut strict = params();
        strict.unmapped = UnmappedPolicy::Fault;
        let mut faulting = Sp804::new(strict);
        assert_eq!(
            faulting.read(&mut ctx, 0x900, AccessSize::Word, &word_attr()),
            Err(BusFault::Device)
        );
        assert_eq!(
            sp804.read(&mut ctx, reg::LOAD, AccessSize::DoubleWord, &word_attr()),
            Err(BusFault::InvalidSize)
        );
        assert_eq!(
            sp804.write(&mut ctx, reg::LOAD, AccessSize::DoubleWord, 0, &word_attr()),
            Err(BusFault::InvalidSize)
        );
    }
}

#[cfg(test)]
mod snapshot_roundtrip {
    use core::num::NonZeroU64;

    use mosaic_core_types::{EventId, SignalId, Snapshot};

    use super::*;

    #[test]
    fn timer_event_ids_restore_with_the_block_state() {
        let mut timer = Sp804::new(Sp804Params::new(
            [SignalId(1), SignalId(2)],
            NonZeroU64::new(1000).unwrap(),
        ));
        timer.timers[0].state.load = 0x100;
        timer.timers[0].event = Some(EventId(7));
        timer.timers[1].irq_asserted = true;
        let saved = timer.save();
        timer.timers[0].event = None;
        timer.timers[0].state.load = 0;
        timer.timers[1].irq_asserted = false;
        timer.load(Sp804::SCHEMA_VERSION, &saved).unwrap();
        assert_eq!(timer.timers[0].state.load, 0x100);
        assert_eq!(timer.timers[0].event, Some(EventId(7)));
        assert!(timer.timers[1].irq_asserted);
    }
}
