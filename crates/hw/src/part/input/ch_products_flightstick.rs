//! CH Products Flightstick gameport
//!
//! Catalog part `part:ch/ch_products_flightstick` — manifests/hw/input/ch_products_flightstick.toml.
//! Thin configuration over the shared gameport stick core (ADR 0027).

use crate::part::input::gameport_stick_core::GamePortStickCore;

/// ch_products_flightstick: 4-axis / 4-button analog gameport stick.
pub fn new() -> GamePortStickCore {
    GamePortStickCore::new()
}
