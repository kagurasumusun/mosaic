//! Corsair ML120 Pro 120 mm magnetic-levitation 4-pin PWM fan (2000 RPM)
//!
//! Catalog part — manifests/hw/misc/corsair-ml120-pro.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// corsair-ml120-pro: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
