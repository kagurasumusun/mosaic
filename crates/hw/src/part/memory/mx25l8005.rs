//! MX25L8005 8Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l8005` — manifests/hw/flash/mx25l8005.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l8005: 1048576 bytes, JEDEC 0xc2 0x20 0x14.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(1048576, [0xc2, 0x20, 0x14])
}
