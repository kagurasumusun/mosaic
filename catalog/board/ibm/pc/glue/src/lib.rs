#![forbid(unsafe_code)]
//! `board:ibm/pc` — the IBM PC/AT-compatible machine, Mosaic's `-M pc`.
//!
//! This crate is the machine *model*, in the catalog where models belong
//! (design §6: `catalog/board/<origin>/<board>`), not inside a front-end
//! application. The structure mirrors QEMU, which the owner asked us to
//! follow:
//!
//! | QEMU | here |
//! | --- | --- |
//! | `softmmu/vl.c` (option parsing, `-M` lookup) | `apps/system` (`mosaic-system`) |
//! | `hw/i386/pc_piix.c`, `hw/i386/pc.c` (the `pc` machine) | this crate, [`machine`] |
//! | `hw/i386/x86.c` `x86_load_linux()` (per-arch `-kernel`) | [`linux_boot`] |
//! | `hw/core/loader.c` (image formats) | `mosaic-modelkit-iso9660` |
//! | `MODULE_INFO("machine")` registration | [`MACHINE_NAME`]/[`CATALOG_ID`] + the front-end's machine list |
//!
//! A front-end never grows machine code when machines are added; it grows one
//! dispatch arm, exactly as QEMU's `vl.c` stays untouched by new `hw/` boards.
//! The next machines (`board:linuxfoundation?/virt` for AArch64, `goldfish`
//! for Android, `versatile-pb` for ARM32) will be sibling board crates under
//! `catalog/board/`, each wiring the catalog IPs and engines it needs.
//!
//! The board declares itself in `../board.toml`; the assembly itself is
//! engine-coupled imperative code here (the declaration's `glue` crate, design
//! §8 R5 as amended by ADR 0023), because boot bring-up needs the engine's
//! port and interrupt hook surfaces. As `describe`/`assemble` grow x86
//! coverage, the imperative wiring narrows toward the declarative graph.

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

/// The i386 Linux boot protocol this machine's `-kernel` runs: `bzImage`
/// parsing and the `boot_params` a loader must build. QEMU's
/// `hw/i386/x86.c::x86_load_linux` role.
pub mod linux_boot;

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
