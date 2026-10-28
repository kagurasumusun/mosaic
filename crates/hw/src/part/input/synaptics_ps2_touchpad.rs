//! Synaptics PS/2 touchpad
//!
//! Catalog part `part:synaptics/synaptics_ps2_touchpad` — manifests/hw/input/synaptics_ps2_touchpad.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::part::input::ps2_mouse_core::Ps2MouseCore;

/// synaptics_ps2_touchpad: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
