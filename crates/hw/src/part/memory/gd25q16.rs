//! GD25Q16 16Mbit SPI flash
//!
//! Catalog part `part:gigadevice/gd25q16` — manifests/hw/flash/gd25q16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// gd25q16: 2097152 bytes, JEDEC 0xc8 0x40 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0xc8, 0x40, 0x15])
}
