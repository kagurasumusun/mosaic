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

use crate::board::{
    build_stm32f103, CaptureCtx, Stm32Board, GPIOA_BASE, RCC_BASE, SRAM_BASE, SRAM_SIZE,
    USART1_BASE,
};

/// Initial stack pointer: top of SRAM (vector-table word 0).
pub const INITIAL_SP: u64 = SRAM_BASE + SRAM_SIZE;
/// Reset handler entry (vector-table word 1 points here with bit 0 set).
pub const RESET_HANDLER: u64 = 0x0000_0008;

const STEPS: usize = 64;
const QUANTUM: u64 = 32;

/// Independent T32 machine code. The literal pool carries the PA5 CRL value
/// `0x4434_4444` (nibble 5 = general-purpose output), USART BRR, and the SCS
/// addresses the guest polls. Vector word 0 is the real initial SP.
static FIRMWARE_WORDS: [u32; 85] = [
    0x20005000, 0x00000009, 0x210469A8, 0x21404308, 0x43080209, 0x484861A8, 0x48486070, 0x20206030,
    0x48476130, 0x200860A0, 0x02092120, 0x60E04308, 0xF000216D, 0x216FF86E, 0xF86BF000, 0xF0002173,
    0x2161F868, 0xF865F000, 0xF0002169, 0x2163F862, 0xF85FF000, 0xF0002120, 0x2165F85C, 0xF859F000,
    0xF000213D, 0x69A8F856, 0xF858F000, 0xF0002120, 0x2173F850, 0xF84DF000, 0xF000213D, 0x6820F84A,
    0xF84CF000, 0xF0002120, 0x216FF844, 0xF841F000, 0xF000213D, 0x68F0F83E, 0xF840F000, 0x2120482A,
    0x68E06001, 0x43082180, 0x482760E0, 0x28206800, 0x2120D1FB, 0xF82DF000, 0xF0002170, 0x213DF82A,
    0xF827F000, 0xF82AF000, 0xF0002120, 0x2169F822, 0xF81FF000, 0xF000213D, 0x481CF81C, 0xF0006800,
    0x68E0F81D, 0x40012180, 0x60E04048, 0x68004817, 0xD1FB2800, 0xF0002120, 0x2171F80C, 0xF809F000,
    0xF000213D, 0xF000F806, 0x210AF809, 0xF801F000, 0x6823E7FE, 0xD5FC061B, 0x47707121, 0x2208B500,
    0x09000701, 0x290A0F09, 0x3937D301, 0x3130E000, 0xFFEFF7FF, 0xD1F33A01, 0x0000BD00, 0x444444B4,
    0x44344444, 0x00000271, 0xE000E104, 0xE000E204, 0xE000ED04,
];

fn firmware() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FIRMWARE_WORDS.len() * 4);
    for word in FIRMWARE_WORDS {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

/// Observed result of one full firmware run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stm32F103Report {
    pub output: Vec<u8>,
    pub hooks: u64,
    pub events: usize,
    pub pin_edges: Vec<bool>,
}

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

/// Expected golden console output for the F103 firmware.
pub const GOLDEN_OUTPUT: &[u8] =
    b"mosaic e=00004004 s=000000C0 o=00000020 p=00000020 i=00435000 q=00000000\n";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_vector_table_and_entry() {
        let image = firmware();
        assert_eq!(image.len(), FIRMWARE_WORDS.len() * 4);
        let word = |offset: usize| -> u32 {
            u32::from_le_bytes(image[offset..offset + 4].try_into().expect("vector word"))
        };
        assert_eq!(word(0), INITIAL_SP as u32);
        assert_eq!(word(4), 0x0000_0009);
    }
}
