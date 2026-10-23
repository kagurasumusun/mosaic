#![forbid(unsafe_code)]
//! Backend-independent A32 smoke fixtures, not a full ISA conformance suite.

use core::num::NonZeroU64;
use mosaic_contract_cpu_engine::{CpuEngine, EngineError, StopReason};
use mosaic_core_types::{Addr, Permissions};
use mosaic_isa_arm::Register;

/// Independently encoded A32 instructions: MOV r0,#42; ADD r1,r0,#1;
/// STR r1,[r2]; LDR r3,[r2]. No external firmware is included.
pub const ARM32_CODE: [u8; 16] = [
    0x2a, 0x00, 0xa0, 0xe3, 0x01, 0x10, 0x80, 0xe2, 0x00, 0x10, 0x82, 0xe5, 0x00, 0x30, 0x92, 0xe5,
];

fn checked<T>(value: Result<T, EngineError>) -> Result<T, String> {
    value.map_err(|e| format!("engine operation failed: {e:?}"))
}

pub fn prepare_arm32(engine: &mut dyn CpuEngine) -> Result<(), String> {
    checked(engine.map_memory(Addr(0x1000), 4096, Permissions::RX))?;
    checked(engine.map_memory(Addr(0x2000), 4096, Permissions::RW))?;
    checked(engine.write_memory(Addr(0x1000), &ARM32_CODE))?;
    checked(engine.reg_write(Register::Pc.into(), 0x1000))?;
    checked(engine.reg_write(Register::R2.into(), 0x2000))?;
    Ok(())
}

pub fn arm32_arithmetic(engine: &mut dyn CpuEngine) -> Result<(), String> {
    prepare_arm32(engine)?;
    let result = engine.run(NonZeroU64::new(4).expect("nonzero fixture budget"));
    if result.reason != StopReason::BudgetExhausted || result.instruction_hooks != 4 {
        return Err(format!("unexpected stop: {result:?}"));
    }
    for (reg, expected) in [
        (Register::R0, 42),
        (Register::R1, 43),
        (Register::R3, 43),
        (Register::Pc, 0x1010),
    ] {
        let actual = checked(engine.reg_read(reg.into()))?;
        if actual != expected {
            return Err(format!("{reg:?}: expected {expected:#x}, got {actual:#x}"));
        }
    }
    let mut memory = [0; 4];
    checked(engine.read_memory(Addr(0x2000), &mut memory))?;
    if memory != 43u32.to_le_bytes() {
        return Err(format!("bad memory result: {memory:?}"));
    }
    Ok(())
}

/// Compare identical inputs under a whole vs partitioned instruction budget.
pub fn arm32_partition(
    first: &mut dyn CpuEngine,
    second: &mut dyn CpuEngine,
) -> Result<(), String> {
    prepare_arm32(first)?;
    prepare_arm32(second)?;
    fn run(engine: &mut dyn CpuEngine, budget: u64) -> Result<(), String> {
        let outcome = engine.run(NonZeroU64::new(budget).expect("nonzero fixture budget"));
        if outcome.reason != StopReason::BudgetExhausted || outcome.instruction_hooks != budget {
            return Err(format!("partition stop: {outcome:?}"));
        }
        Ok(())
    }
    run(first, 4)?;
    for _ in 0..4 {
        run(second, 1)?;
    }
    for reg in [
        Register::R0,
        Register::R1,
        Register::R2,
        Register::R3,
        Register::Pc,
    ] {
        if checked(first.reg_read(reg.into()))? != checked(second.reg_read(reg.into()))? {
            return Err(format!("partition mismatch at {reg:?}"));
        }
    }
    let mut a = [0; 4];
    let mut b = [0; 4];
    checked(first.read_memory(Addr(0x2000), &mut a))?;
    checked(second.read_memory(Addr(0x2000), &mut b))?;
    if a != b {
        return Err("partition memory mismatch".into());
    }
    Ok(())
}
