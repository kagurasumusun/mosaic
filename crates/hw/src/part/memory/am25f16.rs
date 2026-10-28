//! AM25F16 16Mbit SPI flash
//!
//! Catalog part `part:amacro/am25f16` — manifests/hw/flash/am25f16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::memory::spi_nor::SpiNorFlash;

/// am25f16: 2097152 bytes, JEDEC 0x37 0x30 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x37, 0x30, 0x15])
}
