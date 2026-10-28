//! Micron MT29F2G08AAG 2 Gbit SLC NAND (2048+64 B pages)
//!
//! Catalog part — manifests/hw/flash/micron-mt29f2g08.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// micron-mt29f2g08: 2048+64 B pages, 131072 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 131072, 64, 0x2C, 0xDA)
}
