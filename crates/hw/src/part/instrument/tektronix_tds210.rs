//! Tektronix TDS 210 two-channel 60 MHz digital oscilloscope (GPIB option)
//!
//! Catalog part — manifests/hw/misc/tektronix-tds210.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// tektronix-tds210: primary address 1
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(1)
        .with_idn("TEKTRONIX,TDS 210,0,CF:91.1CT FV:v1.11\n")
}
