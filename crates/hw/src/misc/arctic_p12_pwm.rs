//! ARCTIC P12 PWM 120 mm 4-pin fan (1800 RPM, PST)
//!
//! Catalog part — manifests/hw/misc/arctic-p12-pwm.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::misc::pwm_device_core::PwmDeviceCore;

/// arctic-p12-pwm: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
