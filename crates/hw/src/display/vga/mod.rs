//! IBM VGA / VESA Bochs DISPI (QEMU's `hw/display/vga.c` + `vbe.c` roles).
//!
//! The model is split the way the hardware is (ADR 0025):
//!
//! * [`regs`] — the register file the CRTC/sequencer/graphics/attribute
//!   controllers expose through I/O ports `3B0h..3DFh`;
//! * [`vram`] — the 256 KiB video memory, the `A0000h..BFFFFh` apertures
//!   through their mode-driven addressing (planar, chain4, odd/even, text),
//!   and the text-mode screen renderer;
//! * [`vbe`] — the VESA Bochs DISPI extension (`1CEh/1CFh`) that sets
//!   linear-framebuffer resolutions.
//!
//! A machine wires the three: ports to [`regs`], aperture memory to
//! [`vram`], and the LFB to the base [`vbe`] reports — the board's choice,
//! never the model's.

pub mod regs;
pub mod vbe;
pub mod vram;

pub use regs::VgaRegisters;
pub use vbe::VgaVbe;
pub use vram::{VgaMemory, VgaMode};
