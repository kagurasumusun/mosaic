//! W25Q40 4Mbit SPI flash
//!
//! Catalog part `part:winbond/w25q40` — manifests/hw/flash/w25q40.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// w25q40: 524288 bytes, JEDEC 0xef 0x40 0x13.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(524288, [0xef, 0x40, 0x13])
}
