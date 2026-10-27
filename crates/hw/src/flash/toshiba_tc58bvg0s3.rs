//! Toshiba TC58BVG0S3HTA00 4 Gbit SLC NAND (4096+224 B pages)
//!
//! Catalog part — manifests/hw/flash/toshiba-tc58bvg0s3.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::flash::nand_flash_core::NandFlashCore;

/// toshiba-tc58bvg0s3: 4096+224 B pages, 131072 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(4096, 224, 131072, 64, 0x98, 0xD3)
}
