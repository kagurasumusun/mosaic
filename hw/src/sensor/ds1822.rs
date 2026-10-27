//! DS1822 1-Wire digital thermometer (12-bit)
//!
//! Catalog part `part:maxim/ds1822` — manifests/hw/sensor/ds1822.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::sensor::onewire_ds18b20::Ds18b20Core;

/// ds1822: 1-Wire thermometer, family code 0x22.
pub fn new() -> Ds18b20Core {
    Ds18b20Core::new(0x22)
}
