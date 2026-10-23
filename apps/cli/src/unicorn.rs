use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::bind_flat_view;
use mosaic_contract_cpu_engine::{CpuEngine, StopReason};
use mosaic_core_runtime::{FlatView, Machine, Region};
use mosaic_core_types::{Addr, Mapping, MemoryKind, NodeId, Permissions};
use mosaic_engine_unicorn::UnicornArm;
use mosaic_isa_arm::Register;

const CPU: NodeId = NodeId(0);
const ROM: NodeId = NodeId(1);
const RAM: NodeId = NodeId(2);

pub(crate) fn run() -> Result<String, String> {
    // Layout is a runtime-derived FlatView, not a hardcoded engine memory map.
    let view = FlatView::build(&[Region::container(
        Addr(0x1000),
        0x2000,
        0,
        vec![
            Region::leaf(
                Mapping {
                    base: Addr(0),
                    size: 4096,
                    target: ROM,
                    offset: 0,
                    kind: MemoryKind::Rom,
                    permissions: Permissions::RX,
                },
                0,
            ),
            Region::leaf(
                Mapping {
                    base: Addr(4096),
                    size: 4096,
                    target: RAM,
                    offset: 0,
                    kind: MemoryKind::Ram,
                    permissions: Permissions::RW,
                },
                0,
            ),
        ],
    )])
    .map_err(|e| format!("layout: {e:?}"))?;

    let mut machine = Machine::new(view.clone(), 16);
    machine
        .add_backing(ROM, 4096)
        .map_err(|e| format!("backing: {e:?}"))?;
    machine
        .add_backing(RAM, 4096)
        .map_err(|e| format!("backing: {e:?}"))?;
    // Independently encoded MOV/ADD/STR/LDR fixture; not a vendor firmware ROM.
    let code = [
        0x2a, 0, 0xa0, 0xe3, 1, 0x10, 0x80, 0xe2, 0, 0x10, 0x82, 0xe5, 0, 0x30, 0x92, 0xe5,
    ];
    machine
        .loader_write(ROM, 0, &code)
        .map_err(|e| format!("load: {e:?}"))?;

    let mut engine = UnicornArm::new().map_err(|e| format!("init: {e:?}"))?;
    // The FlatView is the single source of truth: native pages and any MMIO
    // callback regions are derived from it by the L5 assembly layer.
    let _machine = bind_flat_view(&mut engine, &view, Arc::new(Mutex::new(machine)), CPU)
        .map_err(|e| format!("bind: {e:?}"))?;

    engine
        .reg_write(Register::Pc.into(), 0x1000)
        .map_err(|e| format!("pc: {e:?}"))?;
    engine
        .reg_write(Register::R2.into(), 0x2000)
        .map_err(|e| format!("r2: {e:?}"))?;
    let result = engine.run(NonZeroU64::new(4).ok_or("invalid fixture budget")?);
    if result.reason != StopReason::BudgetExhausted || result.instruction_hooks != 4 {
        return Err(format!("unexpected stop: {result:?}"));
    }
    let r0 = engine
        .reg_read(Register::R0.into())
        .map_err(|e| format!("r0: {e:?}"))?;
    let r1 = engine
        .reg_read(Register::R1.into())
        .map_err(|e| format!("r1: {e:?}"))?;
    let r3 = engine
        .reg_read(Register::R3.into())
        .map_err(|e| format!("r3: {e:?}"))?;
    let pc = engine
        .reg_read(Register::Pc.into())
        .map_err(|e| format!("pc: {e:?}"))?;
    let mut memory = [0; 4];
    engine
        .read_memory(Addr(0x2000), &mut memory)
        .map_err(|e| format!("memory: {e:?}"))?;
    let memory = u32::from_le_bytes(memory);
    if (r0, r1, r3, pc, memory) != (42, 43, 43, 0x1010, 43) {
        return Err("unexpected guest state".into());
    }
    Ok(format!(
        "engine=unicorn isa=arm-a32 mappings={}\nstop=budget-exhausted hooks={}\nr0={r0} r1={r1} r3={r3} pc={pc:#x}\nmemory[0x2000]={memory}\n",
        view.mappings().len(),
        result.instruction_hooks
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicorn_passes_backend_independent_a32_fixture() {
        mosaic_engine_conformance::arm32_arithmetic(&mut UnicornArm::new().unwrap()).unwrap();
    }

    #[test]
    fn partitioning_instruction_budgets_preserves_registers_and_memory() {
        mosaic_engine_conformance::arm32_partition(
            &mut UnicornArm::new().unwrap(),
            &mut UnicornArm::new().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn smoke_trace_is_repeatable() {
        assert_eq!(run().unwrap(), run().unwrap());
    }
}
