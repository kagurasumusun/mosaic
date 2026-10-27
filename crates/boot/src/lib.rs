#![forbid(unsafe_code)]
//! `boot` — the boot foundation and the image-reading foundation.
//!
//! QEMU splits "make this image boot" across `hw/core/loader.c` (image
//! formats: ELF, uImage, raw), `block/` (disk image formats) and
//! `hw/<arch>/*.c` (per-arch boot protocols: `x86_load_linux`,
//! `arm_load_kernel`, RISC-V boot). Mosaic puts the whole *decoding* half in
//! this crate, so a machine model only owns placement and register
//! conventions:
//!
//! | module | role |
//! | --- | --- |
//! | [`loader`] | kernel image container formats (QEMU `hw/core/loader.c`): ELF, uImage, PE/COFF, SREC/IHEX, multiboot headers, raw + auto-detect |
//! | [`format`] | disk and filesystem image formats: ISO9660 + El Torito, MBR/GPT partition tables, FAT12/16/32, initramfs cpio |
//! | [`proto`] | per-architecture boot protocols (QEMU `hw/<arch>/boot.c`): x86 Linux `boot_params`, ARM32 Linux + ATAGS, AArch64 Linux + FDT, RISC-V Linux, Cortex-M vector table, multiboot |
//!
//! Rules that keep this crate at "QEMU-equivalent or better" (ADR 0025):
//!
//! * **decode only** — no runtime, no engines, no devices, no machines:
//!   every function takes bytes and returns typed structure, so a front-end
//!   (`-kernel <iso>!<path>`), any machine model and the boot tests all read
//!   images through one parser;
//! * **no synthesis** — nothing here invents bytes a real loader would not
//!   have; a boot protocol module builds exactly the structures its
//!   specification assigns to a boot loader (`boot_params`, ATAGS, FDT
//!   fixups), which is a loader's documented job;
//! * **reject, never guess** — every format check that fails is an error
//!   naming the offset and the expected magic, the way a real bootloader
//!   stops rather than booting garbage.

pub mod format;
pub mod loader;
pub mod proto;
