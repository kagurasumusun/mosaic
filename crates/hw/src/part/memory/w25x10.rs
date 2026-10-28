//! W25X10 1Mbit SPI flash
//!
//! Catalog part `part:winbond/w25x10` — manifests/hw/flash/w25x10.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// w25x10: 131072 bytes, JEDEC 0xef 0x40 0x11.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(131072, [0xef, 0x40, 0x11])
}
