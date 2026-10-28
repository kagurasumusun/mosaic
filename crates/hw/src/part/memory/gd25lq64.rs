//! GD25LQ64 64Mbit low-voltage SPI flash
//!
//! Catalog part `part:gigadevice/gd25lq64` — manifests/hw/flash/gd25lq64.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// gd25lq64: 8388608 bytes, JEDEC 0xc8 0x40 0x17.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(8388608, [0xc8, 0x40, 0x17])
}
