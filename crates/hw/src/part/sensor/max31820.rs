//! MAX31820 1-Wire temperature sensor (12-bit)
//!
//! Catalog part `part:maxim/max31820` — manifests/hw/sensor/max31820.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::sensor::onewire_ds18b20::Ds18b20Core;

/// max31820: 1-Wire thermometer, family code 0x3B.
pub fn new() -> Ds18b20Core {
    Ds18b20Core::new(0x3B)
}
