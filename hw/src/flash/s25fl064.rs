//! S25FL064 64Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl064` — manifests/hw/flash/s25fl064.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl064: 8388608 bytes, JEDEC 0x01 0x02 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0x01, 0x02, 0x17])
}
