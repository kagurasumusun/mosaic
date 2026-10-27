//! XT25F64 64Mbit SPI flash
//!
//! Catalog part `part:xtx/xt25f64` — manifests/hw/flash/xt25f64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// xt25f64: 8388608 bytes, JEDEC 0x0b 0x60 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0x0b, 0x60, 0x17])
}
