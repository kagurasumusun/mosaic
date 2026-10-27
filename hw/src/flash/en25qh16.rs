//! EN25QH16 16Mbit SPI flash
//!
//! Catalog part `part:eon/en25qh16` — manifests/hw/flash/en25qh16.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::flash::spi_nor::SpiNorFlash;

/// en25qh16: 2097152 bytes, JEDEC 0x1c 0x30 0x15.
pub fn new() -> SpiNorFlash {
    SpiNorFlash::new(2097152, [0x1c, 0x30, 0x15])
}
