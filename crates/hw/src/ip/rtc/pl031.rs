//! The ARM PrimeCell Real Time Clock (PL031) — QEMU's `hw/rtc/pl031.c` role.
//!
//! Clean-room from ARM DDI 0222: the data register (seconds), the match
//! register and its raw/clear status (with mask), the load register, the
//! interrupt clear register, and the PrimeCell ID registers the probe
//! protocol reads. The clock ticks at 1 Hz in virtual time: the model
//! schedules its next second boundary through [`Ctx::schedule`] and raises
//! the interrupt when the counter matches.

use crate::modelkit::unmapped_policy::UnmappedPolicy;
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{set_optional_signal, ContextError, Ctx, EventTarget};
use mosaic_exec::{
    AccessSize, Attr, BusFault, Fnv64, SignalId, Snapshot, SnapshotError, VDelta, VTime,
};

/// Register offsets (ARM DDI 0222 §3.1).
#[allow(dead_code)] // documented register offsets (SIZE included for the manifest)
mod reg {
    pub const DR: u64 = 0x000; // Data register (seconds)
    pub const MR: u64 = 0x004; // Match register
    pub const LR: u64 = 0x008; // Load register
    pub const CR: u64 = 0x00C; // Control register (bit 0: start)
    pub const IMSC: u64 = 0x010; // Interrupt mask set/clear
    pub const RIS: u64 = 0x014; // Raw interrupt status
    pub const MIS: u64 = 0x018; // Masked interrupt status
    pub const ICR: u64 = 0x01C; // Interrupt clear register
    pub const PERIPH_ID0: u64 = 0xFE0;
    pub const PERIPH_ID1: u64 = 0xFE4;
    pub const PERIPH_ID2: u64 = 0xFE8;
    pub const PERIPH_ID3: u64 = 0xFEC;
    pub const PCELL_ID0: u64 = 0xFF0;
    pub const PCELL_ID1: u64 = 0xFF4;
    pub const PCELL_ID2: u64 = 0xFF8;
    pub const PCELL_ID3: u64 = 0xFFC;
    pub const SIZE: u64 = 0x1000;
}

/// One second in picoseconds.
pub const SECOND_PS: u64 = 1_000_000_000_000;

/// The PL031.
#[derive(Debug)]
pub struct Pl031 {
    irq: SignalId,
    /// The epoch the counter reads from: `seconds_at_base` + elapsed seconds
    /// since `base_time`.
    base_time: VTime,
    seconds_at_base: u32,
    match_register: u32,
    control: u32,
    mask: bool,
    raw_status: bool,
    unmapped: UnmappedPolicy,
}

impl Pl031 {
    pub fn new(irq: SignalId) -> Self {
        Self {
            irq,
            base_time: VTime::ZERO,
            seconds_at_base: 0,
            match_register: 0,
            control: 1, // The oscillator runs by default.
            mask: false,
            raw_status: false,
            unmapped: UnmappedPolicy::RazWi,
        }
    }

    fn seconds(&self, now: VTime) -> u32 {
        if self.control & 1 == 0 {
            return self.seconds_at_base;
        }
        let elapsed = now.0.saturating_sub(self.base_time.0) / SECOND_PS;
        self.seconds_at_base.wrapping_add(elapsed as u32)
    }

    #[allow(dead_code)] // the counter sync the load path grows into
    fn rebase(&mut self, now: VTime) {
        self.seconds_at_base = self.seconds(now);
        self.base_time = now;
    }

    fn check_match(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let now = ctx.now();
        let seconds = self.seconds(now);
        if seconds == self.match_register {
            self.raw_status = true;
        }
        set_optional_signal(ctx, self.irq, self.raw_status && self.mask)
    }

    /// Schedule the next 1 Hz tick (token 1).
    fn schedule_tick(&mut self, ctx: &mut dyn Ctx) -> Result<(), ContextError> {
        let now = ctx.now();
        let into_second = (now.0 - self.base_time.0) % SECOND_PS;
        let next = SECOND_PS - into_second;
        ctx.schedule_after(VDelta(next.max(1)), 1)?;
        Ok(())
    }
}

impl MmioTarget for Pl031 {
    fn read(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        Ok(match offset {
            reg::DR => u64::from(self.seconds(ctx.now())),
            reg::MR => u64::from(self.match_register),
            reg::LR => u64::from(self.seconds_at_base),
            reg::CR => u64::from(self.control),
            reg::IMSC => u64::from(self.mask as u32),
            reg::RIS => u64::from(self.raw_status as u32),
            reg::MIS => u64::from((self.raw_status && self.mask) as u32),
            reg::PERIPH_ID0 => 0x31,
            reg::PERIPH_ID1 => 0x10,
            reg::PERIPH_ID2 => 0x14,
            reg::PERIPH_ID3 => 0x00,
            reg::PCELL_ID0 => 0x0D,
            reg::PCELL_ID1 => 0xF0,
            reg::PCELL_ID2 => 0x05,
            reg::PCELL_ID3 => 0xB1,
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => 0,
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        })
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        offset: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        if size != AccessSize::Word {
            return Err(BusFault::InvalidSize);
        }
        match offset {
            reg::DR => {}
            reg::MR => {
                self.match_register = value as u32;
                self.check_match(ctx).ok();
            }
            reg::LR => {
                // Load the counter with the written seconds.
                self.seconds_at_base = value as u32;
                self.base_time = ctx.now();
                self.check_match(ctx).ok();
            }
            reg::CR => self.control = value as u32 & 1,
            reg::IMSC => {
                self.mask = value & 1 != 0;
                set_optional_signal(ctx, self.irq, self.raw_status && self.mask).ok();
            }
            reg::ICR => {
                if value & 1 != 0 {
                    self.raw_status = false;
                    set_optional_signal(ctx, self.irq, false).ok();
                }
            }
            _ => match self.unmapped {
                UnmappedPolicy::RazWi => {}
                UnmappedPolicy::Fault => return Err(BusFault::Device),
            },
        }
        Ok(())
    }
}

impl EventTarget for Pl031 {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token == 1 {
            self.check_match(ctx).ok();
            self.schedule_tick(ctx).ok();
        }
        Ok(())
    }
}

/// Checkpoint state (ADR 0004).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pl031Checkpoint {
    pub schema: u32,
    pub base_time: u64,
    pub seconds_at_base: u32,
    pub match_register: u32,
    pub control: u32,
    pub mask: bool,
    pub raw_status: bool,
}

impl Snapshot for Pl031 {
    type State = Pl031Checkpoint;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        Self::State {
            schema: Self::SCHEMA_VERSION,
            base_time: self.base_time.0,
            seconds_at_base: self.seconds_at_base,
            match_register: self.match_register,
            control: self.control,
            mask: self.mask,
            raw_status: self.raw_status,
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
        self.base_time = VTime(state.base_time);
        self.seconds_at_base = state.seconds_at_base;
        self.match_register = state.match_register;
        self.control = state.control;
        self.mask = state.mask;
        self.raw_status = state.raw_status;
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = Fnv64::default();
        hash.bytes(&version.to_le_bytes());
        hash.bytes(&state.seconds_at_base.to_le_bytes());
        hash.bytes(&state.match_register.to_le_bytes());
        hash.bytes(&state.control.to_le_bytes());
        hash.bytes(&state.base_time.to_le_bytes());
        hash.bytes(&[state.mask as u8, state.raw_status as u8]);
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
        signals: Vec<(SignalId, bool)>,
        scheduled: Vec<(VTime, u64)>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                now_ps: 0,
                signals: Vec::new(),
                scheduled: Vec::new(),
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
            security: Security::Secure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0,
            endian: Endian::Little,
        }
    }

    #[test]
    fn the_primecell_ids_read_back() {
        let mut rtc = Pl031::new(SignalId(1));
        let mut ctx = TestCtx::new();
        assert_eq!(
            rtc.read(&mut ctx, reg::PERIPH_ID0, AccessSize::Word, &attr())
                .unwrap(),
            0x31
        );
        assert_eq!(
            rtc.read(&mut ctx, reg::PCELL_ID0, AccessSize::Word, &attr())
                .unwrap(),
            0x0D
        );
    }

    #[test]
    fn the_counter_advances_at_1hz_and_matches() {
        let mut rtc = Pl031::new(SignalId(5));
        let mut ctx = TestCtx::new();
        // Enable the match interrupt at second 3.
        rtc.write(&mut ctx, reg::MR, AccessSize::Word, 3, &attr())
            .unwrap();
        rtc.write(&mut ctx, reg::IMSC, AccessSize::Word, 1, &attr())
            .unwrap();
        ctx.now_ps = 3 * SECOND_PS;
        rtc.on_timer(&mut ctx, 1).unwrap();
        assert_eq!(
            rtc.read(&mut ctx, reg::RIS, AccessSize::Word, &attr())
                .unwrap(),
            1
        );
        assert_eq!(
            rtc.read(&mut ctx, reg::MIS, AccessSize::Word, &attr())
                .unwrap(),
            1
        );
        assert!(ctx.signals.contains(&(SignalId(5), true)));
        // ICR clears.
        rtc.write(&mut ctx, reg::ICR, AccessSize::Word, 1, &attr())
            .unwrap();
        assert_eq!(
            rtc.read(&mut ctx, reg::MIS, AccessSize::Word, &attr())
                .unwrap(),
            0
        );
    }

    #[test]
    fn the_load_register_sets_the_counter() {
        let mut rtc = Pl031::new(SignalId(1));
        let mut ctx = TestCtx::new();
        rtc.write(&mut ctx, reg::LR, AccessSize::Word, 1_000_000, &attr())
            .unwrap();
        ctx.now_ps = SECOND_PS;
        assert_eq!(
            rtc.read(&mut ctx, reg::DR, AccessSize::Word, &attr())
                .unwrap(),
            1_000_001
        );
    }
}
