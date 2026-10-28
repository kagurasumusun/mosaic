//! MX25L1605 16Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l1605` — manifests/hw/flash/mx25l1605.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l1605: 2097152 bytes, JEDEC 0xc2 0x20 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0xc2, 0x20, 0x15])
}
