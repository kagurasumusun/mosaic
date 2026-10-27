//! S25FL256 256Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl256` — manifests/hw/flash/s25fl256.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl256: 33554432 bytes, JEDEC 0x01 0x02 0x19.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(33554432, [0x01, 0x02, 0x19])
}
