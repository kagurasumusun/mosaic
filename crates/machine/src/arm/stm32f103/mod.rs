#![forbid(unsafe_code)]
//! `stm32f103` — the STM32F103 machine, Mosaic's `-M stm32f103`.
//!
//! QEMU's `hw/arm/stm32*_soc.c` + board role: the RM0008 memory map comes
//! from the machine's SoC declaration (`manifests/machine/arm/stm32f103/`),
//! the composition from [`board`], and the engines are the accel backends.
//! There is no synthetic firmware in this machine any more: `-kernel` is the
//! only boot source, the image's own Cortex-M vector table supplies the
//! initial SP/PC, and nothing preloads peripheral addresses into registers
//! (the old demo harness did; a real image would be lied to).
//!
//! Boot protocol (Cortex-M reset sequence as QEMU's `armv7m_load_kernel`
//! performs it): word 0 of the image is the initial `SP`, word 1 (with the
//! Thumb bit masked off) is the reset handler, both read from the image
//! itself.

pub mod board;
pub mod manifest;

#[cfg(feature = "unicorn")]
pub mod unicorn_run;

use crate::MachineInfo;

/// The catalog identity of this machine's SoC.
pub const CATALOG_ID: &str = "soc:st/stm32f103";

/// The name a system front-end selects this machine by (`-M stm32f103`).
pub const MACHINE_NAME: &str = "stm32f103";

/// One-line description, as `-M help` prints it.
pub const MACHINE_SUMMARY: &str =
    "STM32F103 Cortex-M3 (RM0008 map: RCC, GPIOA, USART1, NVIC; -kernel boots a raw image)";

/// CPU architectures the machine's engines cover.
pub const ARCHITECTURES: &[&str] = &["armv7m"];

/// The `-M help` listing entry (QEMU's `MODULE_INFO` metadata).
pub fn info() -> MachineInfo {
    MachineInfo {
        name: MACHINE_NAME,
        summary: MACHINE_SUMMARY,
        architectures: ARCHITECTURES,
    }
}

/// Run controls, the M-profile counterpart of the PC machine's budget knobs
/// (QEMU's `-icount` role): an instruction budget and the scheduling quantum.
#[derive(Clone, Copy, Debug)]
pub struct Stm32f103Options {
    /// Total instruction budget. A machine this small deserves a smaller
    /// default than a PC, but the front end decides.
    pub instruction_limit: u64,
    /// Instructions per scheduling quantum. The RCC clock gate is delivered
    /// as a queued signal, so 1 is the value that keeps peripheral bring-up
    /// observable in order.
    pub quantum: u64,
}

impl Default for Stm32f103Options {
    fn default() -> Self {
        Self {
            instruction_limit: 20_000_000,
            quantum: 1,
        }
    }
}

/// What one run observed: the USART1 console, how much executed, and the
/// PA5 pin probe the board wires for GPIO bring-up diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stm32F103Report {
    /// Bytes the guest wrote to the console.
    pub output: Vec<u8>,
    /// Instruction hooks the engine reported (one per executed instruction
    /// at quantum 1).
    pub instructions: u64,
    /// Runtime events delivered to devices.
    pub events: usize,
    /// Level history the board's PA5 probe recorded.
    pub pin_edges: Vec<bool>,
}

/// Read the Cortex-M reset entry (initial SP, reset-handler PC) out of the
/// image's own vector table. The reset sequence belongs to the boot crate
/// (`mosaic_boot::proto::cortex_m`, QEMU's `armv7m_load_kernel` role,
/// ADR 0025); this machine's `-kernel` protocol is exactly it.
pub fn vector_entry(image: &[u8]) -> Result<(u64, u64), String> {
    mosaic_boot::proto::cortex_m::vector_entry(image).map_err(|detail| format!("-kernel: {detail}"))
}

/// Run the machine on the Unicorn backend (`-M stm32f103`). ADR 0030:
/// Unicorn is the only CPU execution engine, so this is the one path.
#[cfg(feature = "unicorn")]
pub fn run(image: &[u8], options: &Stm32f103Options) -> Result<Stm32F103Report, String> {
    unicorn_run::run_stm32f103(image, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_name_matches_the_catalog_id() {
        assert!(CATALOG_ID.ends_with(MACHINE_NAME));
    }

    #[test]
    fn vector_entry_reads_sp_and_pc_like_the_reset_sequence() {
        let mut image = vec![0u8; 16];
        image[0..4].copy_from_slice(&0x2000_5000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0000_0009u32.to_le_bytes());
        assert_eq!(vector_entry(&image), Ok((0x2000_5000, 0x0000_0008)));
    }

    #[test]
    fn vector_entry_rejects_a_table_that_is_not_there() {
        assert!(vector_entry(&[0u8; 4]).is_err());
        assert!(vector_entry(&[0u8; 8]).is_err());
    }
}
