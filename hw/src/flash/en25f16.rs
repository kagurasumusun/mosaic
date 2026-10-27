//! EN25F16 16Mbit SPI flash
//!
//! Catalog part `part:eon/en25f16` — manifests/hw/flash/en25f16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// en25f16: 2097152 bytes, JEDEC 0x1c 0x30 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x1c, 0x30, 0x15])
}
