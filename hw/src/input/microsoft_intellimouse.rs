//! Microsoft IntelliMouse PS/2 wheel mouse
//!
//! Catalog part `part:microsoft/microsoft_intellimouse` — manifests/hw/input/microsoft_intellimouse.toml.
//! Thin configuration over the shared PS/2 mouse core (ADR 0027).

use crate::input::ps2_mouse_core::Ps2MouseCore;

/// microsoft_intellimouse: standard PS/2 mouse with 3-byte packets and IntelliMouse wheel
/// extension negotiation.
pub fn new() -> Ps2MouseCore {
    Ps2MouseCore::new()
}
