//! SK hynix HY27UF082G2M 2 Gbit SLC NAND (2048+64 B pages)
//!
//! Catalog part — manifests/hw/flash/hynix-hy27uf082g2m.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::part::memory::nand_flash_core::NandFlashCore;

/// hynix-hy27uf082g2m: 2048+64 B pages, 131072 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 131072, 64, 0xAD, 0xDA)
}
