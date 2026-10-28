//! Microsoft Basic Mouse 2.0 PS/2
//!
//! Catalog part `part:microsoft/microsoft_basic_mouse` — manifests/hw/input/microsoft_basic_mouse.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::part::input::ps2_mouse_core::Ps2MouseCore;

/// microsoft_basic_mouse: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
