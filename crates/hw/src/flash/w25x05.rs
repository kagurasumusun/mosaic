//! W25X05 512Kbit SPI flash
//!
//! Catalog part `part:winbond/w25x05` — manifests/hw/flash/w25x05.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// w25x05: 65536 bytes, JEDEC 0xef 0x40 0x10.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(65536, [0xef, 0x40, 0x10])
}
