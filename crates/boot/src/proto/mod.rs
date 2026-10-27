//! Per-architecture boot protocols (QEMU's `hw/<arch>/x86.c::x86_load_linux`,
//! `hw/arm/boot.c::arm_load_kernel`, `hw/riscv/boot.c` roles).
//!
//! A boot protocol module owns the *documented loader-side* structure of its
//! architecture: where images are placed, which registers carry facts, and
//! which tables a loader must build (`boot_params`, ATAGS, FDT fixups). It
//! decodes only — placement into guest memory is the machine model's job,
//! exactly as QEMU's machine-specific loaders call these helpers.
//!
//! | module | protocol |
//! | --- | --- |
//! | [`x86_linux`] | the i386 Linux boot protocol (`bzImage`, `boot_params`) |
//! | [`arm_linux`] + [`atags`] | ARM32 Linux boot conventions (r0/r1/r2, ATAGS) |
//! | [`arm64_linux`] | AArch64 Linux boot conventions (x0 = DTB, `text_offset`) |
//! | [`riscv_linux`] | RISC-V Linux boot conventions (a0 = hartid, a1 = DTB) |
//! | [`cortex_m`] | the Cortex-M reset sequence (vector table word 0/1) |
//! | [`multiboot`] | the multiboot 1 information structure |
//! | [`dtb`] | FDT (device tree blob) parsing and loader-side fixups |

pub mod arm64_linux;
pub mod arm_linux;
pub mod atags;
pub mod cortex_m;
pub mod dtb;
pub mod multiboot;
pub mod riscv_linux;
pub mod x86_linux;
