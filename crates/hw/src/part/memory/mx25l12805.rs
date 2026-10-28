//! MX25L12805 128Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l12805` — manifests/hw/flash/mx25l12805.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l12805: 16777216 bytes, JEDEC 0xc2 0x20 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0xc2, 0x20, 0x18])
}
