//! W25Q64 64Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q64` — manifests/hw/flash/w25q64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// w25q64: 8388608 bytes, JEDEC 0xef 0x40 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0xef, 0x40, 0x17])
}
