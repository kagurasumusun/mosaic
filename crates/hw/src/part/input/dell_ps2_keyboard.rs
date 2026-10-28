//! Dell SK-8125 PS/2 keyboard
//!
//! Catalog part `part:dell/dell_ps2_keyboard` — manifests/hw/input/dell_ps2_keyboard.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::part::input::ps2_keyboard_core::Ps2KeyboardCore;

/// dell_ps2_keyboard: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
