//! XT25F32 32Mbit SPI flash
//!
//! Catalog part `part:xtx/xt25f32` — manifests/hw/flash/xt25f32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// xt25f32: 4194304 bytes, JEDEC 0x0b 0x60 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x0b, 0x60, 0x16])
}
