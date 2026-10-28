//! IBM TrackPoint II PS/2 pointing device
//!
//! Catalog part `part:ibm/ibm_trackpoint_ii` — manifests/hw/input/ibm_trackpoint_ii.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::part::input::ps2_mouse_core::Ps2MouseCore;

/// ibm_trackpoint_ii: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
