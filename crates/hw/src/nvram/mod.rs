//! Non-volatile memory / firmware interfaces (QEMU `hw/nvram/`).

pub mod catalog;
pub mod fw_cfg;

pub mod lpc11xx_flash;
pub mod option_bytes_f1;
pub mod pretec_sram_4mb;
