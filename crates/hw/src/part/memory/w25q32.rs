//! W25Q32 32Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q32` — manifests/hw/flash/w25q32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// w25q32: 4194304 bytes, JEDEC 0xef 0x40 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0xef, 0x40, 0x16])
}
