//! AArch64 `virt64` Unicorn harness.
//!
//! The machine is built from the SoC declaration (ADR 0016, ADR 0033
//! wave 3). This module loads the `-kernel` image and steps the AArch64
//! engine. Exception entry still needs an injection-capable backend
//! (ADR 0009: Unicorn 2.1.x has none for AArch64), so the harness writes
//! `PC` itself — from the image's own `Image` header (the AArch64 boot
//! protocol) — and the guest observes the GICv3 at register level.
//!
//! The harness stops on:
//!
//! * the instruction budget running out;
//! * the engine reporting `Fault` (an unmapped access, an undefined
//!   instruction, or a synchronous exception the guest did not handle);
//! * the engine reporting `Halted` (a guest that explicitly asked to
//!   stop, which bare-metal test images do).
//!
//! The harness does not yet generate a Device Tree Blob: a real Linux
//! kernel needs one, and an image that probes `x0` for the DTB pointer
//! will read zero. The board's PL011 console captures whatever the guest
//! writes through the host attachment.

use core::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use mosaic_accel_unicorn::UnicornArm64;
use mosaic_assemble::{bind_flat_view_paged, MachineDriver};
use mosaic_contract::cpu_engine::{CpuEngine, RunOutcome, StopReason};
use mosaic_contract::stream::ByteStream;
use mosaic_isa_aarch64::Register;

use super::board::{build_virt64, CaptureCtx, RAM_BASE};
use super::{image_entry, Virt64Options, Virt64Report};

const CONSOLE_BUFFER: usize = 4096;
/// The AArch64 Linux boot protocol's load alignment: 2 MiB above the RAM
/// base (the boot protocol's "image is loaded at a 2 MiB-aligned base
/// within the first 512 MiB of RAM").
const KERNEL_LOAD_BASE: u64 = RAM_BASE + 0x0008_0000;

/// Build and run the `virt64` machine once over a real `-kernel` image.
pub fn run_virt64(image: &[u8], options: &Virt64Options) -> Result<Virt64Report, String> {
    let (entry_in_image, text_offset) = image_entry(image)?;
    let board = build_virt64(options.ram_bytes)?;
    let load_base = KERNEL_LOAD_BASE;
    let entry = load_base + text_offset + entry_in_image;

    let mut machine = board.machine;
    let view = board.view;
    let mut console = board.console;
    let core = board.core;

    // Loader write: the image lands at `load_base` in guest RAM. The
    // machine's RAM backing accepts the bytes; the engine picks them up
    // through the shared view after binding.
    let ram = board.ram;
    machine
        .loader_write(ram, 0, image)
        .map_err(|error| format!("load: {error:?}"))?;

    let mut engine = UnicornArm64::new().map_err(|error| format!("engine: {error:?}"))?;
    let shared = bind_flat_view_paged(&mut engine, &view, Arc::new(Mutex::new(machine)), core)
        .map_err(|error| format!("bind: {error:?}"))?;

    // AArch64 boot protocol (Documentation/arch/arm64/booting.rst):
    // x0 = DTB physical address, all other registers undefined, MMU off,
    // caches off, PC = kernel entry. We do not yet generate a DTB, so x0
    // stays zero — a bare-metal image that does not probe x0 boots
    // normally; a Linux kernel that probes x0 will see "no DTB" and
    // halt cleanly with a recognisable message.
    engine
        .reg_write(Register::X0.into(), 0)
        .map_err(|error| format!("x0: {error:?}"))?;
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
                // Unicorn 2.1.x does not inject AArch64 external
                // interrupts (ADR 0009). A WFI here is the guest waiting
                // for an interrupt the harness cannot deliver — stop and
                // report, exactly as a board without an interrupt
                // controller would.
                stop_reason = "waiting-for-interrupt (AArch64 external IRQ injection not available in Unicorn 2.1.x; ADR 0009)".to_owned();
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
    Ok(Virt64Report {
        output: buffer,
        instructions: hooks,
        events,
        stop_reason,
    })
}
