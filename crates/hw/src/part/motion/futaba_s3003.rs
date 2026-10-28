//! Futaba S3003 standard analog RC servo (PWM 50 Hz, 3.2 kg-cm)
//!
//! Catalog part — manifests/hw/misc/futaba-s3003.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// futaba-s3003: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
