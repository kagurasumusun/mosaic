//! MX25L4005 4Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l4005` — manifests/hw/flash/mx25l4005.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// mx25l4005: 524288 bytes, JEDEC 0xc2 0x20 0x13.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(524288, [0xc2, 0x20, 0x13])
}
