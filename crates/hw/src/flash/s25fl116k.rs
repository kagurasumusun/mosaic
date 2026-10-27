//! S25FL116K 16Mbit SPI flash
//!
//! Catalog part `part:cypress/s25fl116k` — manifests/hw/flash/s25fl116k.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl116k: 2097152 bytes, JEDEC 0x01 0x02 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x01, 0x02, 0x15])
}
