//! RISC-V `virt` Unicorn harness.
//!
//! The machine is built from the SoC declaration (ADR 0016, ADR 0033
//! wave 3). This module loads the `-kernel` image and steps the RV64
//! engine. Exception entry still needs an injection-capable backend
//! (ADR 0009: Unicorn 2.1.x has none for RISC-V), so the harness writes
//! `PC` itself — from the boot protocol's kernel placement — and the
//! guest observes the PLIC and CLINT at register level.
//!
//! The harness does not yet generate a Device Tree Blob: a real Linux
//! kernel needs one, and an image that probes `a1` for the DTB pointer
//! will read zero. The board's PL011 console captures whatever the guest
//! writes through the host attachment.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_accel_unicorn::UnicornRiscV64;
use mosaic_assemble::{bind_flat_view_paged, MachineDriver};
use mosaic_contract::cpu_engine::{CpuEngine, RunOutcome, StopReason};
use mosaic_contract::stream::ByteStream;
use mosaic_isa_riscv::Register;

use super::board::{build_virt, CaptureCtx, RAM_BASE};
use super::{VirtOptions, VirtReport};

const CONSOLE_BUFFER: usize = 4096;
/// The RISC-V Linux boot protocol's load alignment: 2 MiB above the RAM
/// base (QEMU's `riscv_load_kernel` convention).
const KERNEL_LOAD_BASE: u64 = RAM_BASE + 0x0020_0000;

/// Build and run the `virt` machine once over a real `-kernel` image.
pub fn run_virt(image: &[u8], options: &VirtOptions) -> Result<VirtReport, String> {
    let board = build_virt(options.ram_bytes)?;
    let load_base = KERNEL_LOAD_BASE;
    let entry = load_base;

    let mut machine = board.machine;
    let view = board.view;
    let mut console = board.console;
    let core = board.core;

    // Loader write: the image lands at `load_base` in guest RAM.
    let ram = board.ram;
    machine
        .loader_write(ram, 0, image)
        .map_err(|error| format!("load: {error:?}"))?;

    let mut engine = UnicornRiscV64::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view_paged(&mut engine, &view, Arc::new(Mutex::new(machine)), core)
        .map_err(|error| format!("bind: {error:?}"))?;

    // RISC-V Linux boot protocol (Documentation/arch/riscv/boot.rst):
    // a0 = hart ID, a1 = DTB physical address, PC = kernel entry. We do
    // not yet generate a DTB, so a1 stays zero — a bare-metal image that
    // does not probe a1 boots normally.
    engine
        .reg_write(Register::A0.into(), 0)
        .map_err(|error| format!("a0: {error:?}"))?;
    engine
        .reg_write(Register::A1.into(), 0)
        .map_err(|error| format!("a1: {error:?}"))?;
    engine
        .reg_write(Register::Pc.into(), entry)
        .map_err(|error| format!("pc: {error:?}"))?;

    let mut driver = MachineDriver::new(engine, shared);
    let quantum = NonZeroU64::new(options.quantum.max(1)).ok_or("invalid quantum")?;
    let mut hooks = 0;
    let mut events = 0;
    let mut stop_reason = "budget-exhausted".to_owned();
    while hooks < options.instruction_limit {
        let report = driver
            .step(quantum, 32)
            .map_err(|error| format!("step: {error:?}"))?;
        let advanced = report.engine.instruction_hooks;
        hooks += advanced;
        events += report.events_delivered;
        let RunOutcome { reason, .. } = report.engine;
        match reason {
            StopReason::BudgetExhausted => {}
            StopReason::WaitingForInterrupt => {
                // Unicorn 2.1.x does not inject RISC-V external
                // interrupts (ADR 0009). A WFI here is the guest waiting
                // for an interrupt the harness cannot deliver — stop and
                // report.
                stop_reason = "waiting-for-interrupt (RISC-V external IRQ injection not available in Unicorn 2.1.x; ADR 0009)".to_owned();
                break;
            }
            StopReason::Halted => {
                stop_reason = "halted".to_owned();
                break;
            }
            StopReason::Fault => {
                stop_reason = "fault".to_owned();
                break;
            }
            StopReason::Breakpoint => {
                stop_reason = "breakpoint".to_owned();
                break;
            }
            StopReason::CapabilityMissing => {
                stop_reason = "capability-missing".to_owned();
                break;
            }
            _ => {
                stop_reason = format!("stop-reason: {reason:?}");
                break;
            }
        }
        if advanced == 0 {
            stop_reason = "no-progress".to_owned();
            break;
        }
    }

    let mut buffer = vec![0u8; CONSOLE_BUFFER];
    let read = console
        .read_bytes(&mut CaptureCtx, &mut buffer)
        .map_err(|error| format!("console read: {error:?}"))?;
    buffer.truncate(read);
    Ok(VirtReport {
        output: buffer,
        instructions: hooks,
        events,
        stop_reason,
    })
}
