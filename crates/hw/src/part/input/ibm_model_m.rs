//! IBM Model M 101-key PS/2 keyboard (1985)
//!
//! Catalog part `part:ibm/ibm_model_m` — manifests/hw/input/ibm_model_m.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::part::input::ps2_keyboard_core::Ps2KeyboardCore;

/// ibm_model_m: default AT identify (0xAB 0x41)
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new()
}
