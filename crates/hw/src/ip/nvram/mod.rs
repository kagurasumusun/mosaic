//! Firmware-facing NVRAM IP blocks (QEMU `hw/nvram/`); NVRAM chips
//! live in `crate::part::memory`.
pub mod catalog;
pub mod fw_cfg;
pub mod lpc11xx_flash;
pub mod option_bytes_f1;
