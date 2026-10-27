//! Micron MT29F4G08AAA 4 Gbit SLC NAND (4096+128 B pages)
//!
//! Catalog part — manifests/hw/flash/micron-mt29f4g08.toml.
//! Thin configuration over the shared raw-NAND flash core (ADR 0029).

use crate::flash::nand_flash_core::NandFlashCore;

/// micron-mt29f4g08: 4096+128 B pages, 131072 blocks
pub fn new() -> NandFlashCore {
    NandFlashCore::with_geometry(4096, 128, 131072, 64, 0x2C, 0xDC)
}
