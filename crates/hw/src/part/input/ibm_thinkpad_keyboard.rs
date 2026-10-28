//! IBM ThinkPad T42 integrated PS/2 keyboard
//!
//! Catalog part `part:ibm/ibm_thinkpad_keyboard` — manifests/hw/input/ibm_thinkpad_keyboard.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::part::input::ps2_keyboard_core::Ps2KeyboardCore;

/// ibm_thinkpad_keyboard: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
