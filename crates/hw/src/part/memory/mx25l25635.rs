//! MX25L25635 256Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l25635` — manifests/hw/flash/mx25l25635.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l25635: 33554432 bytes, JEDEC 0xc2 0x20 0x19.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(33554432, [0xc2, 0x20, 0x19])
}
