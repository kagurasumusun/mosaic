#![forbid(unsafe_code)]
//! `virt` — the RISC-V `virt` machine, Mosaic's `-M riscv-virt`.
//!
//! QEMU's `hw/riscv/virt.c` role: the `VIRT_MEMMAP` map comes from the
//! SoC declaration (`manifests/machine/riscv/virt/soc.toml`), the
//! composition from [`board`], and the engine is the Unicorn RV64
//! backend (ADR 0030, ADR 0033 wave 3). The board owns only what boards
//! own (ADR 0025): the declaration, the host console endpoint for
//! `soc.uart0`, and the runtime RAM size override (QEMU's `-m`).
//!
//! Boot protocol (RISC-V Linux, QEMU's `hw/riscv/boot.c` role): the
//! kernel is placed on a 2 MiB-aligned address 2 MiB above the RAM base;
//! `a0` carries the booting hart's ID (0 for a single-hart machine),
//! `a1` carries the device tree blob's physical address. The DTB itself
//! is currently the harness's responsibility (a real image's FDT
//! generation is a follow-up wave); bare-metal `-kernel` images that do
//! not need a DTB boot directly.

pub mod board;
pub mod manifest;

#[cfg(all(feature = "unicorn", feature = "arch-riscv"))]
pub mod unicorn_run;

use crate::MachineInfo;

/// The catalog identity of this machine's SoC.
pub const CATALOG_ID: &str = "soc:riscv/virt";

/// The name a system front-end selects this machine by (`-M riscv-virt`).
pub const MACHINE_NAME: &str = "riscv-virt";

/// One-line description, as `-M help` prints it.
pub const MACHINE_SUMMARY: &str =
    "RISC-V virt (SiFive U54 RV64, CLINT, PLIC, PL011, virtio-mmio; -kernel boots a flat image)";

/// CPU architectures the machine's engines cover.
pub const ARCHITECTURES: &[&str] = &["riscv64"];

/// The `-M help` listing entry (QEMU's `MODULE_INFO` metadata).
pub fn info() -> MachineInfo {
    MachineInfo {
        name: MACHINE_NAME,
        summary: MACHINE_SUMMARY,
        architectures: ARCHITECTURES,
    }
}

/// Run controls, the RV64 counterpart of the PC machine's budget knobs
/// (QEMU's `-icount` role): an instruction budget and the scheduling
/// quantum.
#[derive(Clone, Copy, Debug)]
pub struct VirtOptions {
    /// Total instruction budget.
    pub instruction_limit: u64,
    /// Instructions per scheduling quantum.
    pub quantum: u64,
    /// Runtime RAM size in bytes (overrides the SoC declaration's
    /// canonical 128 MiB when set; QEMU's `-m` role).
    pub ram_bytes: u64,
}

impl Default for VirtOptions {
    fn default() -> Self {
        Self {
            instruction_limit: 200_000_000,
            quantum: 50_000,
            ram_bytes: 128 * 1024 * 1024,
        }
    }
}

/// What one run observed: the PL011 console, how much executed, and the
/// runtime events delivered to devices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VirtReport {
    /// Bytes the guest wrote to the console.
    pub output: Vec<u8>,
    /// Instruction hooks the engine reported.
    pub instructions: u64,
    /// Runtime events delivered to devices.
    pub events: usize,
    /// The reason the run stopped, in the engine's own words.
    pub stop_reason: String,
}

/// Run the machine on the Unicorn backend (`-M riscv-virt`). ADR 0030:
/// Unicorn is the only CPU execution engine, so this is the one path.
/// The `arch-riscv` feature must be enabled at build time; without it,
/// the unicorn crate does not include the RV64 backend.
#[cfg(all(feature = "unicorn", feature = "arch-riscv"))]
pub fn run(image: &[u8], options: &VirtOptions) -> Result<VirtReport, String> {
    unicorn_run::run_virt(image, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_name_does_not_collide_with_the_aarch64_virt() {
        // QEMU's `virt` is selected by `-M virt` and disambiguated by the
        // target triple. Mosaic's front-end is single-binary, so the
        // RISC-V virt gets a `riscv-` prefix to stay distinct from
        // `virt64`.
        assert_ne!(MACHINE_NAME, "virt");
        assert_ne!(MACHINE_NAME, "virt64");
        assert!(CATALOG_ID.ends_with("virt"));
    }
}
