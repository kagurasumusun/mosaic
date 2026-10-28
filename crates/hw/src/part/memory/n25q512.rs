//! N25Q512 512Mbit SPI flash
//!
//! Catalog part `part:micron/n25q512` — manifests/hw/flash/n25q512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// n25q512: 67108864 bytes, JEDEC 0x20 0xba 0x1a.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(67108864, [0x20, 0xba, 0x1a])
}
