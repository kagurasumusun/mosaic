//! MX25L3205 32Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l3205` — manifests/hw/flash/mx25l3205.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l3205: 4194304 bytes, JEDEC 0xc2 0x20 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0xc2, 0x20, 0x16])
}
