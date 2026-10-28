//! Keithley 2000 6.5-digit integrating digital multimeter (GPIB)
//!
//! Catalog part — manifests/hw/misc/keithley-2000.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// keithley-2000: primary address 16
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(16)
        .with_idn("KEITHLEY INSTRUMENTS INC.,MODEL 2000,0,A30/A30\n")
}
