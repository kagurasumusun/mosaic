#![forbid(unsafe_code)]
//! `virt64` — the AArch64 `virt` machine, Mosaic's `-M virt64`.
//!
//! QEMU's `hw/arm/virt.c` role: the `VIRT_MEMMAP` map comes from the SoC
//! declaration (`manifests/machine/arm/virt64/soc.toml`), the composition
//! from [`board`], and the engine is the Unicorn AArch64 backend
//! (ADR 0030). The board owns only what boards own (ADR 0025): the
//! declaration, the host console endpoint for `soc.uart0`, and the
//! runtime RAM size override (QEMU's `-m`).
//!
//! Boot protocol (AArch64 Linux `Image`, QEMU's `hw/arm/boot.c` AArch64
//! path): the kernel is placed `text_offset` bytes above a 2 MiB-aligned
//! base within the first 512 MiB of RAM; `x0` carries the device tree
//! blob's physical address. The DTB itself is currently the harness's
//! responsibility (a real image's FDT generation is a follow-up wave);
//! bare-metal `-kernel` images that do not need a DTB boot directly.

pub mod board;
pub mod manifest;

#[cfg(feature = "unicorn")]
pub mod unicorn_run;

use crate::MachineInfo;

/// The catalog identity of this machine's SoC.
pub const CATALOG_ID: &str = "soc:arm/virt64";

/// The name a system front-end selects this machine by (`-M virt64`).
pub const MACHINE_NAME: &str = "virt64";

/// One-line description, as `-M help` prints it.
pub const MACHINE_SUMMARY: &str =
    "AArch64 virt (Cortex-A53, GICv3, PL011, virtio-mmio; -kernel boots an arm64 Image)";

/// CPU architectures the machine's engines cover.
pub const ARCHITECTURES: &[&str] = &["aarch64"];

/// The `-M help` listing entry (QEMU's `MODULE_INFO` metadata).
pub fn info() -> MachineInfo {
    MachineInfo {
        name: MACHINE_NAME,
        summary: MACHINE_SUMMARY,
        architectures: ARCHITECTURES,
    }
}

/// Run controls, the AArch64 counterpart of the PC machine's budget knobs
/// (QEMU's `-icount` role): an instruction budget and the scheduling
/// quantum.
#[derive(Clone, Copy, Debug)]
pub struct Virt64Options {
    /// Total instruction budget. A real Linux boot is in the hundreds of
    /// millions of instructions; the harness ceiling is the operator's.
    pub instruction_limit: u64,
    /// Instructions per scheduling quantum.
    pub quantum: u64,
    /// Runtime RAM size in bytes (overrides the SoC declaration's
    /// canonical 128 MiB when set; QEMU's `-m` role).
    pub ram_bytes: u64,
}

impl Default for Virt64Options {
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
pub struct Virt64Report {
    /// Bytes the guest wrote to the console.
    pub output: Vec<u8>,
    /// Instruction hooks the engine reported.
    pub instructions: u64,
    /// Runtime events delivered to devices.
    pub events: usize,
    /// The reason the run stopped, in the engine's own words.
    pub stop_reason: String,
}

/// Parse the AArch64 `Image` header and return the entry point (the
/// branch target at word 0) and the `text_offset` the image requests.
/// The full header decode lives in the boot crate
/// (`mosaic_boot::proto::arm64_linux`, QEMU's `hw/arm/boot.c` AArch64
/// role, ADR 0025); this machine's `-kernel` protocol is exactly it.
pub fn image_entry(image: &[u8]) -> Result<(u64, u64), String> {
    let header = mosaic_boot::proto::arm64_linux::parse_header(image)
        .ok_or_else(|| "-kernel: not an arm64 Image (header magic mismatch)".to_owned())?;
    // The branch at word 0 is a relative `b` instruction; its target is
    // the entry point relative to the image's load base. The boot crate
    // does not yet expose the decoded target, so the machine reads the
    // AArch64 offset form (bits 23..0 sign-extended, shifted left 2).
    let word0 = u32::from_le_bytes(image[0..4].try_into().expect("header length checked"));
    let imm24 = (word0 & 0x00ff_ffff) as i32;
    let sign_extended = (imm24 << 8) >> 8;
    let entry = (sign_extended as i64).wrapping_mul(4) as u64;
    Ok((entry, header.text_offset))
}

/// Run the machine on the Unicorn backend (`-M virt64`). ADR 0030:
/// Unicorn is the only CPU execution engine, so this is the one path.
#[cfg(feature = "unicorn")]
pub fn run(image: &[u8], options: &Virt64Options) -> Result<Virt64Report, String> {
    unicorn_run::run_virt64(image, options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_machine_name_matches_the_catalog_id() {
        assert!(CATALOG_ID.ends_with(MACHINE_NAME));
    }

    /// The smallest valid arm64 Image: a `b .` self-loop at word 0
    /// (offset 0, target = 0), and the `text_offset`/`image_size`/`flags`
    /// fields the boot crate parses. The harness uses this shape to
    /// verify the entry decode without inventing a fake guest.
    #[test]
    fn image_entry_reads_a_self_loop_as_offset_zero() {
        let mut image = vec![0u8; 0x20];
        // `b .`: 0x14000000 — branch to self (offset 0).
        image[0..4].copy_from_slice(&0x1400_0000u32.to_le_bytes());
        // text_offset = 0x0 (PIE image, no offset).
        image[0x08..0x10].copy_from_slice(&0u64.to_le_bytes());
        // image_size = 0x20.
        image[0x10..0x18].copy_from_slice(&0x20u64.to_le_bytes());
        // flags = FLAG_PIE_4G (bit 3).
        image[0x18..0x20].copy_from_slice(&(1u64 << 3).to_le_bytes());
        let (entry, text_offset) = image_entry(&image).expect("arm64 Image");
        assert_eq!(entry, 0);
        assert_eq!(text_offset, 0);
    }

    #[test]
    fn image_entry_rejects_a_too_short_image() {
        assert!(image_entry(&[0u8; 8]).is_err());
    }
}
