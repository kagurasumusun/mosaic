//! AM25F64 64Mbit SPI flash
//!
//! Catalog part `part:amacro/am25f64` — manifests/hw/flash/am25f64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// am25f64: 8388608 bytes, JEDEC 0x37 0x30 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0x37, 0x30, 0x17])
}
