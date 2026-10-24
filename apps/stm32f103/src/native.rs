//! Run the STM32F103 image on the native ARMv7-M core.
//!
//! The machine is the same declaration the Unicorn harness uses. This path
//! does not link Unicorn: MMIO goes through [`MachineBus`] and the cooperative
//! driver, and the core executes Thumb itself.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{MachineBus, MachineDriver};
use mosaic_contract_cpu_engine::CpuEngine;
use mosaic_contract_stream::ByteStream;
use mosaic_core_types::MemoryKind;
use mosaic_engine_native_armv7m::NativeCortexM3;
use mosaic_isa_arm::Register;

use crate::board::{build_stm32f103, CaptureCtx, GPIOA_BASE, RCC_BASE, USART1_BASE};
use crate::image::{firmware, Stm32F103Report, INITIAL_SP, RESET_HANDLER};

// One instruction per step. RCC's gate is a queued signal (ADR 0001); a larger
// quantum would let the guest program USART before the enable is delivered.
const STEPS: usize = 2048;
const QUANTUM: u64 = 1;

/// Build the declared F103 machine and execute the image on [`NativeCortexM3`].
pub fn run_stm32f103_native() -> Result<Stm32F103Report, String> {
    let mut board = build_stm32f103()?;
    board
        .machine
        .loader_write(board.flash, 0, &firmware())
        .map_err(|error| format!("load: {error:?}"))?;

    let machine = Arc::new(Mutex::new(board.machine));
    let mut engine = NativeCortexM3::new();
    engine
        .attach_bus(Box::new(MachineBus::new(Arc::clone(&machine), board.core)))
        .map_err(|error| format!("bus: {error:?}"))?;
    for mapping in board.view.mappings() {
        match mapping.kind {
            MemoryKind::Ram | MemoryKind::Rom => {
                engine
                    .map_memory(mapping.base, mapping.size, mapping.permissions)
                    .map_err(|error| format!("map {:#x}: {error:?}", mapping.base.0))?;
                let length = usize::try_from(mapping.size).map_err(|_| "region too large")?;
                let mut bytes = vec![0u8; length];
                machine
                    .lock()
                    .map_err(|_| "machine poisoned")?
                    .loader_read(mapping.target, mapping.offset, &mut bytes)
                    .map_err(|error| format!("copy {:#x}: {error:?}", mapping.base.0))?;
                engine
                    .write_memory(mapping.base, &bytes)
                    .map_err(|error| format!("write {:#x}: {error:?}", mapping.base.0))?;
            }
            MemoryKind::Mmio => {
                engine
                    .map_mmio(mapping.base, mapping.size)
                    .map_err(|error| format!("mmio {:#x}: {error:?}", mapping.base.0))?;
            }
        }
    }
    engine
        .reg_write(Register::Sp.into(), INITIAL_SP)
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), RESET_HANDLER)
        .map_err(|error| format!("pc: {error:?}"))?;
    engine
        .reg_write(Register::R4.into(), USART1_BASE)
        .map_err(|error| format!("r4: {error:?}"))?;
    engine
        .reg_write(Register::R5.into(), RCC_BASE)
        .map_err(|error| format!("r5: {error:?}"))?;
    engine
        .reg_write(Register::R6.into(), GPIOA_BASE)
        .map_err(|error| format!("r6: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, machine);
    let quantum = NonZeroU64::new(QUANTUM).ok_or("invalid quantum")?;
    let mut hooks = 0;
    let mut events = 0;
    for _ in 0..STEPS {
        let report = driver
            .step(quantum, 32)
            .map_err(|error| format!("step: {error:?}"))?;
        hooks += report.engine.instruction_hooks;
        events += report.events_delivered;
    }

    let mut console = board.console;
    let mut buffer = vec![0u8; 128];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(Stm32F103Report {
        output: buffer,
        hooks,
        events,
        pin_edges: board
            .pin_levels
            .lock()
            .map(|levels| levels.clone())
            .map_err(|_| "pin probe poisoned")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::GOLDEN_OUTPUT;

    #[test]
    fn native_core_prints_the_golden_console() {
        let first = run_stm32f103_native().expect("native run");
        let second = run_stm32f103_native().expect("native rerun");
        assert_eq!(first, second, "native execution must be deterministic");
        assert_eq!(
            first.output,
            GOLDEN_OUTPUT,
            "console {}",
            String::from_utf8_lossy(&first.output)
        );
        assert_eq!(first.pin_edges, vec![false, true]);
    }
}
