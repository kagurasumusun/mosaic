//! XT25F128 128Mbit SPI flash
//!
//! Catalog part `part:xtx/xt25f128` — manifests/hw/flash/xt25f128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// xt25f128: 16777216 bytes, JEDEC 0x0b 0x60 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0x0b, 0x60, 0x18])
}
