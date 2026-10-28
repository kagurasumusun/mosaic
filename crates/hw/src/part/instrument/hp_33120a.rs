//! HP 33120A 15 MHz function/arbitrary waveform generator (GPIB)
//!
//! Catalog part — manifests/hw/misc/hp-33120a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// hp-33120a: primary address 10
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(10)
        .with_idn("HEWLETT-PACKARD,33120A,0,4.0-4.0\n")
}
