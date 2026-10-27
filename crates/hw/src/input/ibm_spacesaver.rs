//! IBM Space Saver 84-key PS/2 keyboard
//!
//! Catalog part `part:ibm/ibm_spacesaver` — manifests/hw/input/ibm_spacesaver.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// ibm_spacesaver: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
