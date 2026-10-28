//! ST NAND01GR3B 1 Gbit SLC NAND (2048+64 B pages, 8-bit bus)
//!
//! Catalog part — manifests/hw/flash/st-nand01gr3b.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// st-nand01gr3b: 2048+64 B pages, 65536 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 65536, 64, 0x20, 0xF1)
}
