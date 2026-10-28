//! Rigol DM3068 6.5-digit digital multimeter (GPIB, USB)
//!
//! Catalog part — manifests/hw/misc/rigol-dm3068.toml.
//! Thin configuration over the shared GPIB instrument core (ADR 0029).

use crate::part::instrument::gpib_instrument_core::GpibInstrumentCore;

/// rigol-dm3068: primary address 18
pub fn new() -> GpibInstrumentCore {
    GpibInstrumentCore::new()
        .at(18)
        .with_idn("Rigol Technologies,DM3068,DM3A19350001,00.02.01\n")
}
