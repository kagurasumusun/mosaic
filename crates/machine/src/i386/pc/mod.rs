#![forbid(unsafe_code)]
//! `pc` — the IBM PC/AT-compatible machine, Mosaic's `-M pc`.
//!
//! This module is the machine *model* under `machine/src/i386/` (QEMU's
//! `hw/i386/pc_piix.c` + `hw/i386/pc.c`), not front-end code (ADR 0024):
//!
//! | QEMU | here |
//! | --- | --- |
//! | `system/vl.c` (option parsing, `-M` lookup) | the `mosaic` front-end crate |
//! | `hw/i386/pc_piix.c`, `hw/i386/pc.c` (the `pc` machine) | this module, [`machine`] |
//! | `hw/i386/x86.c` `x86_load_linux()` (per-arch `-kernel`) | [`linux_boot`] |
//! | `hw/core/loader.c` (image formats) | `block::iso9660` |
//! | `MODULE_INFO("machine")` registration | [`MACHINE_NAME`]/[`CATALOG_ID`] + [`info`] in the machine registry |
//!
//! A front-end never grows machine code when machines are added: it grows one
//! dispatch arm, exactly as QEMU's `vl.c` stays untouched by new `hw/` boards.
//! The next machines (`virt` for AArch64, `goldfish` for Android) will be
//! sibling modules under `machine/src/<arch>/`, each wiring the `hw` devices
//! and `accel` engines it needs.
//!
//! The machine declares itself in `manifests/machine/i386/pc/board.toml`; the
//! assembly itself is engine-coupled imperative code here, because boot
//! bring-up needs the engine's port and interrupt hook surfaces. As
//! `describe`/`assemble` grow x86 coverage, the imperative wiring narrows
//! toward the declarative graph (ADR 0023, superseded in placement by
//! ADR 0024).

/// The catalog identity of this board.
pub const CATALOG_ID: &str = "board:ibm/pc";

/// The name a system front-end selects this machine by (`-M pc`).
pub const MACHINE_NAME: &str = "pc";

/// One-line description, as `-M help` prints it.
pub const MACHINE_SUMMARY: &str = "i386 PC (BIOS INT services, ISA port space, VGA text)";

/// CPU architectures the machine's engines cover.
pub const ARCHITECTURES: &[&str] = &["i386", "x86_64"];

/// A kernel the operator asked the machine to boot directly, bypassing every
/// loader in the attached images (QEMU's `-kernel`/`-initrd`/`-append`). The
/// words are machine-neutral; their meaning is [`linux_boot`]'s to carry out.
#[derive(Clone, Debug)]
pub struct DirectKernel {
    /// `bzImage` bytes.
    pub image: Vec<u8>,
    /// Optional initramfs bytes.
    pub initrd: Option<Vec<u8>>,
    /// Kernel command line.
    pub cmdline: String,
}

/// The i386 Linux boot protocol this machine's `-kernel` runs (bzImage
/// parsing, the `boot_params` a loader must build) lives in the boot crate:
/// `mosaic_boot::proto::x86_linux`, QEMU's `hw/i386/x86.c::x86_load_linux`
/// role (ADR 0025). The machine owns placement and registers only.
/// The machine: BIOS software-interrupt firmware interim, PIC/PIT/RTC port
/// space, VGA text console, El Torito and boot-sector boot, and the run loop.
/// QEMU's `hw/i386/pc_piix.c` + `hw/i386/pc.c` role.
#[cfg(feature = "unicorn")]
pub mod machine;

#[cfg(feature = "unicorn")]
pub use machine::{
    run_pc_machine, BootPoint, DiskImage, DiskRead, DriveMedia, FirmwareImage, KernelPlacement,
    KernelSummary, PcBootOptions, PcBootReport, PcMachineConfig,
};

/// The `-M help` listing entry (QEMU's `MODULE_INFO` metadata).
pub fn info() -> crate::MachineInfo {
    crate::MachineInfo {
        name: MACHINE_NAME,
        summary: MACHINE_SUMMARY,
        architectures: ARCHITECTURES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine identity a front-end selects is exactly the catalog
    /// identity's suffix — `-M pc` and `board:ibm/pc` cannot drift apart.
    #[test]
    fn the_machine_name_matches_the_catalog_id() {
        assert!(CATALOG_ID.ends_with(MACHINE_NAME));
    }
}
