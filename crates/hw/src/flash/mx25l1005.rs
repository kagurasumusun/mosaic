//! MX25L1005 1Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l1005` — manifests/hw/flash/mx25l1005.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// mx25l1005: 131072 bytes, JEDEC 0xc2 0x20 0x11.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(131072, [0xc2, 0x20, 0x11])
}
