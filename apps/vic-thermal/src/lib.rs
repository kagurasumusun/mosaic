#![forbid(unsafe_code)]
//! P3 interrupt-controller composition. The unicorn feature gates the native
//! CPU backend exactly like the development CLI (ADR 0007).

#[cfg(feature = "unicorn")]
mod vic_thermal;

#[cfg(feature = "unicorn")]
pub use vic_thermal::{run_vic_thermal, VicThermalReport, GOLDEN_OUTPUT};
