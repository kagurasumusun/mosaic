//! MX25L51245G 512Mbit SPI flash
//!
//! Catalog part `part:macronix/mx25l51245g` — manifests/hw/flash/mx25l51245g.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// mx25l51245g: 67108864 bytes, JEDEC 0xc2 0x20 0x1a.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(67108864, [0xc2, 0x20, 0x1a])
}
