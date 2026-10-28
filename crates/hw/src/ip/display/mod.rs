//! Display controller IP blocks (QEMU `hw/display/`): VGA, LCD
//! controllers, GPUs. Panels and monitors live in `crate::part::display`.
pub mod ast_gfx;
pub mod catalog;
pub mod dpi;
pub mod dsi;
pub mod gd5446;
pub mod i915;
pub mod ltdc;
pub mod s3c_fb;
pub mod stdvga;
pub mod vbe;
pub mod vga;
