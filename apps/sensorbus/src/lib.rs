#![forbid(unsafe_code)]
//! P2 sensor-bus composition. The unicorn feature gates the native CPU
//! backend exactly like the bring-up app and the CLI (ADR 0007).

#[cfg(feature = "unicorn")]
mod sensorbus;

#[cfg(feature = "unicorn")]
pub use sensorbus::{run_sensorbus, SensorbusReport, GOLDEN_OUTPUT};
