//! TowerPro MG996R metal-gear analog RC servo (PWM 50 Hz, 11 kg-cm)
//!
//! Catalog part — manifests/hw/misc/towerpro-mg996r.toml.
//! Thin configuration over the shared PWM device core (ADR 0029).

use crate::modelkit::pwm_device_core::PwmDeviceCore;

/// towerpro-mg996r: single PWM channel consumer
pub fn new() -> PwmDeviceCore {
    PwmDeviceCore::new()
}
