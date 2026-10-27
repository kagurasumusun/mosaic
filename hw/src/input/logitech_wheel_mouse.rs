//! Logitech Wheel Mouse PS/2
//!
//! Catalog part `part:logitech/logitech_wheel_mouse` — manifests/hw/input/logitech_wheel_mouse.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::input::ps2_mouse_core::Ps2MouseCore;

/// logitech_wheel_mouse: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
