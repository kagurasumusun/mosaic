//! S25FL016 16Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl016` — manifests/hw/flash/s25fl016.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl016: 2097152 bytes, JEDEC 0x01 0x02 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x01, 0x02, 0x15])
}
