//! NMB-MAT 1608FK 40 mm 4-pin PWM blower fan
//!
//! Catalog part — manifests/hw/misc/nmb-1608fk.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::misc::pwm_device_core::PwmDeviceCore;

/// nmb-1608fk: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
