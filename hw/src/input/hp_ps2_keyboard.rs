//! Hewlett-Packard KU-0316 PS/2 keyboard
//!
//! Catalog part `part:hp/hp_ps2_keyboard` — manifests/hw/input/hp_ps2_keyboard.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// hp_ps2_keyboard: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
