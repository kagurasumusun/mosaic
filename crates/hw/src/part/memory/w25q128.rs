//! W25Q128 128Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q128` — manifests/hw/flash/w25q128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// w25q128: 16777216 bytes, JEDEC 0xef 0x40 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0xef, 0x40, 0x18])
}
