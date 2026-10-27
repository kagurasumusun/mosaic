//! HP 54600B 100 MHz 2-channel digital oscilloscope (GPIB)
//!
//! Catalog part — manifests/hw/misc/hp-54600b.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// hp-54600b: primary address 7
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(7)
        .with_idn("HEWLETT-PACKARD,54600B,0,01.00\n")
}
