//! Delta PFM1212DE 120 mm 4-pin PWM server fan (high static pressure)
//!
//! Catalog part — manifests/hw/misc/delta-pfm1212de.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// delta-pfm1212de: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
