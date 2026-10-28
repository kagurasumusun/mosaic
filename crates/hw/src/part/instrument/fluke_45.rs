//! Fluke 45 dual-display 5-digit digital multimeter (GPIB option)
//!
//! Catalog part — manifests/hw/misc/fluke-45.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// fluke-45: primary address 6
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new().at(6).with_idn("FLUKE,45,0,1.0\n")
}
