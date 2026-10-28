//! Memory chips (ADR 0031 §3): serial EEPROM, FRAM, SPI NOR, raw NAND,
//! UFS, HyperFlash/HyperRAM, SRAM cards — the old eeprom/ and flash/
//! split is merged into one domain and the family cores (24Cxx, 25Cxx,
//! 93Cx6, CFI, SPI NOR, NAND) live here too.

use crate::catalog::ModelEntry;

pub const MODELS: &[ModelEntry] = &[];
