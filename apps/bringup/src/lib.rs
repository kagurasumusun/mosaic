#![forbid(unsafe_code)]
//! P1 bring-up composition. The unicorn feature gates the native CPU backend
//! exactly like the development CLI (ADR 0007).

#[cfg(feature = "unicorn")]
mod bringup;

#[cfg(feature = "unicorn")]
pub use bringup::{run_bringup, BringupReport, GOLDEN_OUTPUT};
