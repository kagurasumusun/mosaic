//! EN25QH128 128Mbit SPI flash
//!
//! Catalog part `part:eon/en25qh128` — manifests/hw/flash/en25qh128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// en25qh128: 16777216 bytes, JEDEC 0x1c 0x30 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0x1c, 0x30, 0x18])
}
