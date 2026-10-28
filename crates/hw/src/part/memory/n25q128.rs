//! N25Q128 128Mbit SPI flash
//!
//! Catalog part `part:micron/n25q128` — manifests/hw/flash/n25q128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// n25q128: 16777216 bytes, JEDEC 0x20 0xba 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0x20, 0xba, 0x18])
}
