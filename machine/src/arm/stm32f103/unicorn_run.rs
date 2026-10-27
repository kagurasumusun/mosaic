//! STM32F103 (medium density, Cortex-M3) Unicorn harness.
//!
//! The machine is built from the SoC declaration (ADR 0016). This module
//! loads the `-kernel` image and steps the M-class engine (ADR 0012).
//! Exception entry still needs an injection-capable backend (ADR 0009), so
//! the harness writes SP and PC itself — from the image's own vector table
//! (the Cortex-M reset sequence) — and the guest observes the NVIC at
//! register level.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_accel_unicorn::UnicornCortexM3;
use mosaic_assemble::{bind_flat_view_paged, MachineDriver};
use mosaic_contract::cpu_engine::CpuEngine;
use mosaic_contract::stream::ByteStream;
use mosaic_isa_arm::Register;

use super::board::{build_stm32f103, CaptureCtx, Stm32Board};
use super::{vector_entry, Stm32F103Report, Stm32f103Options};

// The quantum stays at 1: RCC's gate is queued, and a larger quantum lets
// the guest program USART before the enable is delivered.
const CONSOLE_BUFFER: usize = 4096;

/// Build and run the F103 machine once over a real `-kernel` image.
pub fn run_stm32f103(image: &[u8], options: &Stm32f103Options) -> Result<Stm32F103Report, String> {
    let (initial_sp, reset_handler) = vector_entry(image)?;
    let Stm32Board {
        view,
        mut machine,
        mut console,
        pin_levels,
        core,
        flash,
    } = build_stm32f103()?;
    machine
        .loader_write(flash, 0, image)
        .map_err(|error| format!("load: {error:?}"))?;

    let mut engine = UnicornCortexM3::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view_paged(&mut engine, &view, Arc::new(Mutex::new(machine)), core)
        .map_err(|error| format!("bind: {error:?}"))?;
    engine
        .reg_write(Register::Sp.into(), initial_sp)
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), reset_handler)
        .map_err(|error| format!("pc: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
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

    let mut buffer = vec![0u8; CONSOLE_BUFFER];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(Stm32F103Report {
        output: buffer,
        instructions: hooks,
        events,
        pin_edges: pin_levels
            .lock()
            .map(|levels| levels.clone())
            .map_err(|_| "pin probe poisoned")?,
    })
}
