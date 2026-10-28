//! HP E3631A triple-output DC power supply (GPIB, 80 W)
//!
//! Catalog part — manifests/hw/misc/hp-e3631a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// hp-e3631a: primary address 5
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(5)
        .with_idn("HEWLETT-PACKARD,E3631A,0,2.1-5.0\n")
}
