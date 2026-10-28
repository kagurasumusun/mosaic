//! be quiet! Pure Wings 2 120 mm 4-pin PWM fan (1500 RPM)
//!
//! Catalog part — manifests/hw/misc/bequiet-pure-wings-2.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// bequiet-pure-wings-2: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
