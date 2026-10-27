//! S25FL132K 32Mbit SPI flash
//!
//! Catalog part `part:cypress/s25fl132k` — manifests/hw/flash/s25fl132k.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl132k: 4194304 bytes, JEDEC 0x01 0x02 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x01, 0x02, 0x16])
}
