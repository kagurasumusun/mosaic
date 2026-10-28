//! DS18B20 1-Wire digital thermometer (12-bit)
//!
//! Catalog part `part:maxim/ds18b20` — manifests/hw/sensor/ds18b20.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::sensor::onewire_ds18b20::Ds18b20Core;

/// ds18b20: 1-Wire thermometer, family code 0x28.
pub fn new() -> Ds18b20Core {
    Ds18b20Core::new(0x28)
}
