//! Run the STM32F103 image on the native ARMv7-M core.
//!
//! The machine is the same declaration the Unicorn harness uses. This path
//! does not link Unicorn: MMIO goes through [`MachineBus`] and the cooperative
//! driver, and the core executes Thumb itself.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_accel_armv7m::NativeCortexM3;
use mosaic_assemble::{MachineBus, MachineDriver};
use mosaic_contract::cpu_engine::CpuEngine;
use mosaic_contract::stream::ByteStream;
use mosaic_exec::MemoryKind;
use mosaic_isa_arm::Register;

use super::board::{build_stm32f103, CaptureCtx};
use super::{vector_entry, Stm32F103Report, Stm32f103Options};

const CONSOLE_BUFFER: usize = 4096;

/// Build the declared F103 machine and execute the `-kernel` image on
/// [`NativeCortexM3`], SP/PC taken from the image's own vector table.
pub fn run_stm32f103_native(
    image: &[u8],
    options: &Stm32f103Options,
) -> Result<Stm32F103Report, String> {
    let (initial_sp, reset_handler) = vector_entry(image)?;
    let mut board = build_stm32f103()?;
    board
        .machine
        .loader_write(board.flash, 0, image)
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
        .reg_write(Register::Sp.into(), initial_sp)
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), reset_handler)
        .map_err(|error| format!("pc: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, machine);
    let quantum = NonZeroU64::new(options.quantum.max(1)).ok_or("invalid quantum")?;
    let mut hooks = 0;
    let mut events = 0;
    while hooks < options.instruction_limit {
        let report = driver
            .step(quantum, 32)
            .map_err(|error| format!("step: {error:?}"))?;
        let advanced = report.engine.instruction_hooks;
        hooks += advanced;
        events += report.events_delivered;
        if advanced == 0 {
            break;
        }
    }

    let mut console = board.console;
    let mut buffer = vec![0u8; CONSOLE_BUFFER];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(Stm32F103Report {
        output: buffer,
        instructions: hooks,
        events,
        pin_edges: board
            .pin_levels
            .lock()
            .map(|levels| levels.clone())
            .map_err(|_| "pin probe poisoned")?,
    })
}
