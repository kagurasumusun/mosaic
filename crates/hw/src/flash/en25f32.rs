//! EN25F32 32Mbit SPI flash
//!
//! Catalog part `part:eon/en25f32` — manifests/hw/flash/en25f32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// en25f32: 4194304 bytes, JEDEC 0x1c 0x30 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0x1c, 0x30, 0x16])
}
