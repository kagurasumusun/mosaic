//! S25FL512 512Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl512` — manifests/hw/flash/s25fl512.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// s25fl512: 67108864 bytes, JEDEC 0x01 0x02 0x1a.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(67108864, [0x01, 0x02, 0x1a])
}
