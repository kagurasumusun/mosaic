//! HP 3458A 8.5-digit digital multimeter (GPIB, 100 kS/s digitizing)
//!
//! Catalog part — manifests/hw/misc/hp-3458a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// hp-3458a: primary address 22
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(22)
        .with_idn("HP,3458A,0,09.01\n")
}
