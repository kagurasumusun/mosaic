//! Samsung K9F1G08U0E 1 Gbit SLC NAND (2048+64 B pages, 64K blocks)
//!
//! Catalog part — manifests/hw/flash/samsung-k9f1g08u0e.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// samsung-k9f1g08u0e: 2048+64 B pages, 65536 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 65536, 64, 0xEC, 0xF1)
}
