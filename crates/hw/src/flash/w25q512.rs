//! W25Q512 512Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q512` — manifests/hw/flash/w25q512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// w25q512: 67108864 bytes, JEDEC 0xef 0x40 0x1a.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(67108864, [0xef, 0x40, 0x1a])
}
