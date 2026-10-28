//! MX25L6405 64Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l6405` — manifests/hw/flash/mx25l6405.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l6405: 8388608 bytes, JEDEC 0xc2 0x20 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0xc2, 0x20, 0x17])
}
