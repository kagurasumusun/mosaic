//! Logitech MX3200 PS/2 keyboard
//!
//! Catalog part `part:logitech/logitech_mx3200` — manifests/hw/input/logitech_mx3200.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// logitech_mx3200: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
