//! Winbond W29N01HVSINA 1 Gbit SLC NAND (2048+64 B pages)
//!
//! Catalog part — manifests/hw/flash/winbond-w29n01hv.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::flash::nand_flash_core::NandFlashCore;

/// winbond-w29n01hv: 2048+64 B pages, 65536 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(2048, 64, 65536, 64, 0xEF, 0xF1)
}
