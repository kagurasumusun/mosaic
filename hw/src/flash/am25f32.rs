//! AM25F32 32Mbit SPI flash
//!
//! Catalog part `part:amacro/am25f32` — manifests/hw/flash/am25f32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// am25f32: 4194304 bytes, JEDEC 0x37 0x30 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x37, 0x30, 0x16])
}
