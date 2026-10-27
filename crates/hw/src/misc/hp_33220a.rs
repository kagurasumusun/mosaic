//! HP/Agilent 33220A 20 MHz function/arbitrary waveform generator (GPIB)
//!
//! Catalog part — manifests/hw/misc/hp-33220a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// hp-33220a: primary address 10
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(10)
        .with_idn("Agilent Technologies,33220A,0,2.02-2.02\n")
}
