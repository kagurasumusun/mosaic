//! Chicony KB-9810 PS/2 keyboard
//!
//! Catalog part `part:chicony/chicony_ps2` — manifests/hw/input/chicony_ps2.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::part::input::ps2_keyboard_core::Ps2KeyboardCore;

/// chicony_ps2: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
