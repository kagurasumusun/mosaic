//! Hitec HS-311 standard analog RC servo (PWM 50 Hz, 3.0 kg-cm)
//!
//! Catalog part — manifests/hw/misc/hitec-hs-311.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::misc::pwm_device_core::PwmDeviceCore;

/// hitec-hs-311: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
