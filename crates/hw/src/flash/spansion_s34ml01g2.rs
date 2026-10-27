//! Spansion S34ML01G2 1 Gbit SLC NAND (2048+64 B pages)
//!
//! Catalog part — manifests/hw/flash/spansion-s34ml01g2.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::flash::nand_flash_core::NandFlashCore;

/// spansion-s34ml01g2: 2048+64 B pages, 65536 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 65536, 64, 0x01, 0xF1)
}
