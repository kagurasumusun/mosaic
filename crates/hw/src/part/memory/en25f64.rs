//! EN25F64 64Mbit SPI flash
//!
//! Catalog part `part:eon/en25f64` — manifests/hw/flash/en25f64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// en25f64: 8388608 bytes, JEDEC 0x1c 0x30 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0x1c, 0x30, 0x17])
}
