//! Logitech 3-button PS/2 mouse (M-UV55)
//!
//! Catalog part `part:logitech/logitech_ps2_mouse` — manifests/hw/input/logitech_ps2_mouse.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::part::input::ps2_mouse_core::Ps2MouseCore;

/// logitech_ps2_mouse: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
