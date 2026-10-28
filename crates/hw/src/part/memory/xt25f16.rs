//! XT25F16 16Mbit SPI flash
//!
//! Catalog part `part:xtx/xt25f16` — manifests/hw/flash/xt25f16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// xt25f16: 2097152 bytes, JEDEC 0x0b 0x60 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x0b, 0x60, 0x15])
}
