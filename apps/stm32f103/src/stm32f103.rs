//! STM32F103 (medium density, Cortex-M3) engine harness.
//!
//! The machine is built from `catalog/soc/st/stm32f103/soc.toml` (ADR 0016).
//! This module only loads the independent T32 image and steps the M-class
//! engine (ADR 0012). Exception entry still needs an injection-capable
//! backend (ADR 0009), so the harness writes SP and PC itself and the guest
//! observes the NVIC at register level.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_assemble::{bind_flat_view_paged, MachineDriver};
use mosaic_contract_cpu_engine::CpuEngine;
use mosaic_contract_stream::ByteStream;
use mosaic_engine_unicorn::UnicornCortexM3;
use mosaic_isa_arm::Register;

use crate::board::{build_stm32f103, CaptureCtx, Stm32Board, GPIOA_BASE, RCC_BASE, USART1_BASE};
use crate::image::{firmware, INITIAL_SP, RESET_HANDLER};

pub use crate::image::{Stm32F103Report, GOLDEN_OUTPUT};

// One instruction per step, same budget as 64×32. RCC's gate is queued; a
// larger quantum lets the guest program USART before the enable is delivered.
const STEPS: usize = 2048;
const QUANTUM: u64 = 1;

/// Build and run the F103 machine once. Two calls must produce identical
/// reports, which the determinism check in `main` relies on.
pub fn run_stm32f103() -> Result<Stm32F103Report, String> {
    let Stm32Board {
        view,
        mut machine,
        mut console,
        pin_levels,
        core,
        flash,
    } = build_stm32f103()?;
    machine
        .loader_write(flash, 0, &firmware())
        .map_err(|error| format!("load: {error:?}"))?;

    let mut engine = UnicornCortexM3::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view_paged(&mut engine, &view, Arc::new(Mutex::new(machine)), core)
        .map_err(|error| format!("bind: {error:?}"))?;
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

    let mut driver = MachineDriver::new(engine, shared);
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

    let mut buffer = vec![0u8; 128];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(Stm32F103Report {
        output: buffer,
        hooks,
        events,
        pin_edges: pin_levels
            .lock()
            .map(|levels| levels.clone())
            .map_err(|_| "pin probe poisoned")?,
    })
}
