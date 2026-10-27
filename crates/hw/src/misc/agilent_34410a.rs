//! Agilent 34410A 6.5-digit high-performance digital multimeter (GPIB/LAN)
//!
//! Catalog part — manifests/hw/misc/agilent-34410a.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::misc::gpib_instrument_core::GpibInstrumentCore;

/// agilent-34410a: primary address 22
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(22)
        .with_idn("Agilent Technologies,34410A,0,3.02-3.02\n")
}
