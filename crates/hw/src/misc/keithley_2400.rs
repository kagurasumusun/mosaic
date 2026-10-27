//! Keithley 2400 SourceMeter SMU instrument (GPIB, 200 V / 1 A)
//!
//! Catalog part — manifests/hw/misc/keithley-2400.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// keithley-2400: primary address 24
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(24)
        .with_idn("KEITHLEY INSTRUMENTS INC.,MODEL 2400,0,C33/C33\n")
}
