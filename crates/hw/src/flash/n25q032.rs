//! N25Q032 32Mbit SPI flash
//!
//! Catalog part `part:micron/n25q032` — manifests/hw/flash/n25q032.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// n25q032: 4194304 bytes, JEDEC 0x20 0xba 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x20, 0xba, 0x16])
}
