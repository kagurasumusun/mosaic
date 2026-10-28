//! S25FL032 32Mbit SPI flash
//!
//! Catalog part `part:spansion/s25fl032` — manifests/hw/flash/s25fl032.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// s25fl032: 4194304 bytes, JEDEC 0x01 0x02 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x01, 0x02, 0x16])
}
