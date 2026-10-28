//! GD25Q128 128Mbit SPI flash
//!
//! Catalog part `part:gigadevice/gd25q128` — manifests/hw/flash/gd25q128.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// gd25q128: 16777216 bytes, JEDEC 0xc8 0x40 0x18.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(16777216, [0xc8, 0x40, 0x18])
}
