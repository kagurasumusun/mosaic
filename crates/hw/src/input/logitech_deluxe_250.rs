//! Logitech Deluxe 250 PS/2 keyboard
//!
//! Catalog part `part:logitech/logitech_deluxe_250` — manifests/hw/input/logitech_deluxe_250.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// logitech_deluxe_250: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
