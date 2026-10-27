//! MX25L512 512Kbit SPI flash
//!
//! Catalog part `part:macronix/mx25l512` — manifests/hw/flash/mx25l512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// mx25l512: 65536 bytes, JEDEC 0xc2 0x20 0x10.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(65536, [0xc2, 0x20, 0x10])
}
