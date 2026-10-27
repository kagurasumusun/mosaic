//! Sun Type 6 PS/2 keyboard (PC mode)
//!
//! Catalog part `part:sun/sun_type6_ps2` — manifests/hw/input/sun_type6_ps2.toml.
//! Thin configuration over the shared PS/2 keyboard core (ADR 0027).

use crate::input::ps2_keyboard_core::Ps2KeyboardCore;

/// sun_type6_ps2: identify = 0xAB 0x81
pub fn new() -> Ps2KeyboardCore {
    Ps2KeyboardCore::new().with_identify(0xAB, 0x81)
}
