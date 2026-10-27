//! HP 34401A 6.5-digit digital multimeter (GPIB, 1000 readings/s)
//!
//! Catalog part — manifests/hw/misc/hp-34401a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// hp-34401a: primary address 22
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(22)
        .with_idn("HEWLETT-PACKARD,34401A,0,9.0-9.0\n")
}
