//! S25FL127 128Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl127` — manifests/hw/flash/s25fl127.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl127: 16777216 bytes, JEDEC 0x01 0x02 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0x01, 0x02, 0x18])
}
