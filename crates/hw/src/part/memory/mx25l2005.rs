//! MX25L2005 2Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l2005` — manifests/hw/flash/mx25l2005.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l2005: 262144 bytes, JEDEC 0xc2 0x20 0x12.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(262144, [0xc2, 0x20, 0x12])
}
