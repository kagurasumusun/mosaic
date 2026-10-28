//! W25Q16 16Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q16` — manifests/hw/flash/w25q16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// w25q16: 2097152 bytes, JEDEC 0xef 0x40 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0xef, 0x40, 0x15])
}
