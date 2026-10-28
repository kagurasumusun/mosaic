//! NZXT FZ120 120 mm 4-pin RGB PWM fan (1500 RPM)
//!
//! Catalog part — manifests/hw/misc/nzxt-fz120.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// nzxt-fz120: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
