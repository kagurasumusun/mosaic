//! N25Q064 64Mbit SPI flash
//!
//! Catalog part `part:micron/n25q064` — manifests/hw/flash/n25q064.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// n25q064: 8388608 bytes, JEDEC 0x20 0xba 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0x20, 0xba, 0x17])
}
