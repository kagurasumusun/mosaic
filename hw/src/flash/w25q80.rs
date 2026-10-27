//! W25Q80 8Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q80` — manifests/hw/flash/w25q80.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// w25q80: 1048576 bytes, JEDEC 0xef 0x40 0x14.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(1048576, [0xef, 0x40, 0x14])
}
