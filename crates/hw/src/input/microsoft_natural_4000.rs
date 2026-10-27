//! Microsoft Natural Ergonomic 4000 PS/2 keyboard
//!
//! Catalog part `part:microsoft/microsoft_natural_4000` — manifests/hw/input/microsoft_natural_4000.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// microsoft_natural_4000: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
