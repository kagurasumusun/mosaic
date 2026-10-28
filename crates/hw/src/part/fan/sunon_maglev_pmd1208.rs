//! Sunon MagLev PMD1208PQB1-A 120 mm 4-pin PWM fan
//!
//! Catalog part — manifests/hw/misc/sunon-maglev-pmd1208.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// sunon-maglev-pmd1208: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
