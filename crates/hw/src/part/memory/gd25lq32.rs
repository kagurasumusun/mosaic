//! GD25LQ32 32Mbit low-voltage SPI flash
//!
//! Catalog part `part:gigadevice/gd25lq32` — manifests/hw/flash/gd25lq32.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// gd25lq32: 4194304 bytes, JEDEC 0xc8 0x40 0x16.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(4194304, [0xc8, 0x40, 0x16])
}
