#![deny(unsafe_code)]
//! Initial ARM little-endian Unicorn adapter. One instance owns one core.
//! No native handles, callbacks or vendor types are part of the public API.

mod ownership;

use core::num::NonZeroU64;
use mosaic_contract_cpu_engine::{
    CpuEngine, EngineCaps, EngineError, RegId, RunOutcome, StopReason,
};
use mosaic_core_types::{Addr, Permissions};
use ownership::{Counters, OwnedUnicorn};
use unicorn_engine::{
    unicorn_const::{Arch, Mode, Prot},
    RegisterARM, Unicorn,
};

/// Deliberate safety limit on an individual synchronous run, not a timing model.
pub const MAX_QUANTUM: u64 = 1_000_000;
pub const PAGE_SIZE: u64 = 4096;

pub struct UnicornArm {
    inner: OwnedUnicorn,
    mappings: Vec<(u64, u64)>,
}

impl UnicornArm {
    pub fn new() -> Result<Self, EngineError> {
        let mut uc = Unicorn::new_with_data(
            Arch::ARM,
            Mode::ARM | Mode::LITTLE_ENDIAN,
            Counters::default(),
        )
        .map_err(|_| EngineError::Backend)?;
        // Capture-free: no Rc/host reference can escape the engine through a hook.
        // Native count and MAX_QUANTUM bound the addition below to <= 1_000_000.
        uc.add_code_hook(1, 0, |uc, _, _| {
            uc.get_data_mut().instructions += 1;
        })
        .map_err(|_| EngineError::Backend)?;
        Ok(Self {
            inner: OwnedUnicorn(uc),
            mappings: Vec::new(),
        })
    }

    fn register(reg: RegId) -> Result<RegisterARM, EngineError> {
        match reg.0 {
            0 => Ok(RegisterARM::R0),
            1 => Ok(RegisterARM::R1),
            2 => Ok(RegisterARM::R2),
            3 => Ok(RegisterARM::R3),
            4 => Ok(RegisterARM::R4),
            5 => Ok(RegisterARM::R5),
            6 => Ok(RegisterARM::R6),
            7 => Ok(RegisterARM::R7),
            8 => Ok(RegisterARM::R8),
            9 => Ok(RegisterARM::R9),
            10 => Ok(RegisterARM::R10),
            11 => Ok(RegisterARM::R11),
            12 => Ok(RegisterARM::R12),
            13 => Ok(RegisterARM::SP),
            14 => Ok(RegisterARM::LR),
            15 => Ok(RegisterARM::PC),
            _ => Err(EngineError::InvalidRegister),
        }
    }

    fn check_span(&self, base: Addr, size: usize) -> Result<(), EngineError> {
        let end = u128::from(base.0) + size as u128;
        if !self
            .mappings
            .iter()
            .any(|&(start, len)| base.0 >= start && end <= u128::from(start) + u128::from(len))
        {
            return Err(EngineError::Unmapped);
        }
        Ok(())
    }
}

impl CpuEngine for UnicornArm {
    fn caps(&self) -> EngineCaps {
        EngineCaps::COUNTED
    }

    fn run(&mut self, instructions: NonZeroU64) -> RunOutcome {
        let budget = instructions.get();
        let outcome = |reason, instruction_hooks| RunOutcome {
            reason,
            instruction_hooks,
        };
        if budget > MAX_QUANTUM {
            return outcome(StopReason::CapabilityMissing, 0);
        }
        let Ok(pc) = self.inner.0.reg_read(RegisterARM::PC) else {
            return outcome(StopReason::Fault, 0);
        };
        // Unicorn takes Thumb state from bit 0 of begin. Preserve it on resume
        // even though the current conformance suite only covers A32 instructions.
        let Ok(cpsr) = self.inner.0.reg_read(RegisterARM::CPSR) else {
            return outcome(StopReason::Fault, 0);
        };
        let begin = pc | ((cpsr >> 5) & 1);
        self.inner.0.get_data_mut().instructions = 0;
        let result = self.inner.0.emu_start(begin, u64::MAX, 0, budget as usize);
        let observed = self.inner.0.get_data().instructions;
        let reason = if result.is_err() {
            StopReason::Fault
        } else if observed == budget {
            StopReason::BudgetExhausted
        } else {
            // A backend early stop is not proof of a precise WFI/halt reason.
            StopReason::CapabilityMissing
        };
        outcome(reason, observed)
    }

    fn reg_read(&self, reg: RegId) -> Result<u64, EngineError> {
        self.inner
            .0
            .reg_read(Self::register(reg)?)
            .map_err(|_| EngineError::Backend)
    }

    fn reg_write(&mut self, reg: RegId, value: u64) -> Result<(), EngineError> {
        let register = Self::register(reg)?;
        if value > u64::from(u32::MAX) || (reg.0 == 15 && value % 4 != 0) {
            return Err(EngineError::InvalidValue);
        }
        self.inner
            .0
            .reg_write(register, value)
            .map_err(|_| EngineError::Backend)
    }

    fn map_memory(
        &mut self,
        base: Addr,
        size: u64,
        permissions: Permissions,
    ) -> Result<(), EngineError> {
        if size == 0
            || base.0 % PAGE_SIZE != 0
            || size % PAGE_SIZE != 0
            || u128::from(base.0) + u128::from(size) > (1u128 << 32)
        {
            return Err(EngineError::InvalidMapping);
        }
        let end = base.0 + size;
        if self
            .mappings
            .iter()
            .any(|&(start, len)| base.0 < start + len && start < end)
        {
            return Err(EngineError::Overlap);
        }
        let mut prot = Prot::NONE;
        if permissions.read {
            prot |= Prot::READ;
        }
        if permissions.write {
            prot |= Prot::WRITE;
        }
        if permissions.execute {
            prot |= Prot::EXEC;
        }
        self.inner
            .0
            .mem_map(base.0, size, prot)
            .map_err(|_| EngineError::Backend)?;
        self.mappings.push((base.0, size));
        Ok(())
    }

    fn write_memory(&mut self, base: Addr, data: &[u8]) -> Result<(), EngineError> {
        if data.is_empty() {
            return Ok(());
        }
        self.check_span(base, data.len())?;
        self.inner
            .0
            .mem_write(base.0, data)
            .map_err(|_| EngineError::Backend)?;
        // Loader writes bypass guest protection, but must not leave stale TBs.
        self.inner
            .0
            .ctl_remove_cache(base.0, base.0 + data.len() as u64)
            .map_err(|_| EngineError::Backend)
    }

    fn read_memory(&self, base: Addr, data: &mut [u8]) -> Result<(), EngineError> {
        if data.is_empty() {
            return Ok(());
        }
        self.check_span(base, data.len())?;
        self.inner
            .0
            .mem_read(base.0, data)
            .map_err(|_| EngineError::Backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mosaic_isa_arm::Register;

    fn engine() -> UnicornArm {
        let mut engine = UnicornArm::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RWX)
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine
    }

    #[test]
    fn instruction_budget_and_resume_are_exact_for_a32() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
            .unwrap();
        for expected_pc in [0x1004, 0x1008] {
            let result = engine.run(NonZeroU64::new(1).unwrap());
            assert_eq!(result.reason, StopReason::BudgetExhausted);
            assert_eq!(result.instruction_hooks, 1);
            assert_eq!(engine.reg_read(Register::Pc.into()), Ok(expected_pc));
        }
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(42));
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
    }

    #[test]
    fn patching_code_invalidates_translated_blocks() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[1, 0, 0xa0, 0xe3])
            .unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(1));
        engine
            .write_memory(Addr(0x1000), &[2, 0, 0xa0, 0xe3])
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        assert_eq!(engine.reg_read(Register::R0.into()), Ok(2));
    }

    #[test]
    fn guest_permissions_are_not_bypassed_by_loader_access() {
        let mut engine = UnicornArm::new().unwrap();
        engine
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RX)
            .unwrap();
        engine
            .map_memory(Addr(0x2000), PAGE_SIZE, Permissions::R)
            .unwrap();
        // MOV r0,#42; STR r0,[r2]. Loader may initialize RX; guest may not write R.
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 0, 0, 0x82, 0xe5])
            .unwrap();
        engine.reg_write(Register::Pc.into(), 0x1000).unwrap();
        engine.reg_write(Register::R2.into(), 0x2000).unwrap();
        assert_eq!(
            engine.run(NonZeroU64::new(2).unwrap()).reason,
            StopReason::Fault
        );
        let mut data = [0; 4];
        engine.read_memory(Addr(0x2000), &mut data).unwrap();
        assert_eq!(data, [0; 4]);

        let mut no_execute = UnicornArm::new().unwrap();
        no_execute
            .map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW)
            .unwrap();
        no_execute
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3])
            .unwrap();
        no_execute.reg_write(Register::Pc.into(), 0x1000).unwrap();
        assert_eq!(
            no_execute.run(NonZeroU64::new(1).unwrap()).reason,
            StopReason::Fault
        );
    }

    #[test]
    fn an_infinite_guest_loop_stops_at_the_instruction_budget() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0xfe, 0xff, 0xff, 0xea])
            .unwrap(); // B .
        let outcome = engine.run(NonZeroU64::new(20).unwrap());
        assert_eq!(outcome.reason, StopReason::BudgetExhausted);
        assert_eq!(outcome.instruction_hooks, 20);
        assert_eq!(engine.reg_read(Register::Pc.into()), Ok(0x1000));
    }

    #[test]
    fn mapped_pages_are_private_to_each_instance() {
        let mut first = engine();
        let mut second = engine();
        first.write_memory(Addr(0x1100), &[1, 2, 3, 4]).unwrap();
        second.write_memory(Addr(0x1100), &[5, 6, 7, 8]).unwrap();
        let mut bytes = [0; 4];
        first.read_memory(Addr(0x1100), &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        second.read_memory(Addr(0x1100), &mut bytes).unwrap();
        assert_eq!(bytes, [5, 6, 7, 8]);
    }

    #[test]
    fn mapping_and_register_errors_do_not_panic() {
        let mut engine = engine();
        assert_eq!(
            engine.map_memory(Addr(1), PAGE_SIZE, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.map_memory(Addr(0x2000), 0, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.map_memory(Addr(0x1000), PAGE_SIZE, Permissions::RW),
            Err(EngineError::Overlap)
        );
        assert_eq!(
            engine.map_memory(Addr(0xffff_f000), 2 * PAGE_SIZE, Permissions::RW),
            Err(EngineError::InvalidMapping)
        );
        assert_eq!(
            engine.reg_read(RegId(99)),
            Err(EngineError::InvalidRegister)
        );
        assert_eq!(
            engine.reg_write(Register::R0.into(), 1 << 32),
            Err(EngineError::InvalidValue)
        );
        assert_eq!(
            engine.reg_write(Register::Pc.into(), 0x1001),
            Err(EngineError::InvalidValue)
        );
        assert_eq!(
            engine.write_memory(Addr(0x2000), &[0]),
            Err(EngineError::Unmapped)
        );
        assert_eq!(
            engine.read_memory(Addr(0x1fff), &mut [0; 2]),
            Err(EngineError::Unmapped)
        );
    }

    #[test]
    fn faults_and_unsupported_budgets_do_not_report_success() {
        let mut engine = UnicornArm::new().unwrap();
        assert_eq!(
            engine.run(NonZeroU64::new(1).unwrap()).reason,
            StopReason::Fault
        );
        assert_eq!(
            engine.run(NonZeroU64::new(MAX_QUANTUM + 1).unwrap()),
            RunOutcome {
                reason: StopReason::CapabilityMissing,
                instruction_hooks: 0,
            }
        );
        assert!(!engine.caps().exclusive_monitor);
        assert!(!engine.caps().precise_exceptions);
    }

    #[test]
    fn a_created_and_used_engine_can_move_between_threads_sequentially() {
        let mut engine = engine();
        engine
            .write_memory(Addr(0x1000), &[0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2])
            .unwrap();
        engine.run(NonZeroU64::new(1).unwrap());
        let mut engine = std::thread::spawn(move || {
            assert_eq!(
                engine.run(NonZeroU64::new(1).unwrap()).reason,
                StopReason::BudgetExhausted
            );
            engine
        })
        .join()
        .unwrap();
        assert_eq!(engine.reg_read(Register::R1.into()), Ok(43));
        engine.write_memory(Addr(0x1100), &[7]).unwrap();
    }
}
