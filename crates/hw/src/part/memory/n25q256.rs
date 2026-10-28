//! N25Q256 256Mbit SPI flash
//!
//! Catalog part `part:micron/n25q256` — manifests/hw/flash/n25q256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// n25q256: 33554432 bytes, JEDEC 0x20 0xba 0x19.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(33554432, [0x20, 0xba, 0x19])
}
