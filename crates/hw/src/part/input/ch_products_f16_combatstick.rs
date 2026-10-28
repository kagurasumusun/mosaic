//! CH Products F-16 Combatstick gameport
//!
//! Catalog part `part:ch/ch_products_f16_combatstick` — manifests/hw/input/ch_products_f16_combatstick.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// ch_products_f16_combatstick: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
