//! TowerPro SG90 9 g micro analog RC servo (PWM 50 Hz)
//!
//! Catalog part — manifests/hw/misc/towerpro-sg90.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// towerpro-sg90: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
