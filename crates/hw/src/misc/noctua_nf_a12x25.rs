//! Noctua NF-A12x25 PWM 120 mm 4-pin fan (2000 RPM, 25 mm depth)
//!
//! Catalog part — manifests/hw/misc/noctua-nf-a12x25.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::misc::pwm_device_core::PwmDeviceCore;

/// noctua-nf-a12x25: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
