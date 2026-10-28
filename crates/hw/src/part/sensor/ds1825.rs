//! DS1825 1-Wire digital thermometer (12-bit)
//!
//! Catalog part `part:maxim/ds1825` — manifests/hw/sensor/ds1825.toml.
//! Thin configuration over a shared functional core (ADR 0026 step 2).

use crate::part::sensor::onewire_ds18b20::Ds18b20Core;

/// ds1825: 1-Wire thermometer, family code 0x3B.
pub fn new() -> Ds18b20Core {
    Ds18b20Core::new(0x3B)
}
