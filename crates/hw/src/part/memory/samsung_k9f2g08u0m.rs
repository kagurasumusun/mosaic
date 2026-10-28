//! Samsung K9F2G08U0M 2 Gbit SLC NAND (2048+64 B pages, 128K blocks)
//!
//! Catalog part — manifests/hw/flash/samsung-k9f2g08u0m.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// samsung-k9f2g08u0m: 2048+64 B pages, 131072 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 131072, 64, 0xEC, 0xDA)
}
