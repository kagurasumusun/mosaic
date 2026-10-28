//! W25X20 2Mbit SPI flash
//!
//! Catalog part `part:winbond/w25x20` — manifests/hw/flash/w25x20.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// w25x20: 262144 bytes, JEDEC 0xef 0x40 0x12.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(262144, [0xef, 0x40, 0x12])
}
