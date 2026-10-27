//! MX25LW51245G 512Mbit SPI flash (quad/octal)
//!
//! Catalog part `part:macronix/mx25lw51245g` — manifests/hw/flash/mx25lw51245g.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// mx25lw51245g: 67108864 bytes, JEDEC 0xc2 0x20 0x1a.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(67108864, [0xc2, 0x20, 0x1a])
}
