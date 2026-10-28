//! Macronix MX30LF1G18AA 1 Gbit SLC NAND (2048+64 B pages)
//!
//! Catalog part — manifests/hw/flash/macronix-mx30lf1g18aa.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// macronix-mx30lf1g18aa: 2048+64 B pages, 65536 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 65536, 64, 0xC2, 0xF1)
}
