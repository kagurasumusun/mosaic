//! DS18S20 1-Wire digital thermometer (9-bit)
//!
//! Catalog part `part:maxim/ds18s20` — manifests/hw/sensor/ds18s20.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::sensor::onewire_ds18b20::Ds18b20Core;

/// ds18s20: 1-Wire thermometer, family code 0x10.
pub fn new() -> Ds18b20Core {
    Ds18b20Core::new(0x10)
}
