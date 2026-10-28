#![forbid(unsafe_code)]
//! `hw` — hardware models, classified by responsibility (ADR 0031):
//!
//! * `ip/<function>/` — CPU-addressed register models: peripherals and
//!   host controllers the SoC or board places into an address space
//!   (QEMU's `hw/<function>/`), one file (or directory) per model;
//! * `part/<domain>/` — fabric-mediated leaf parts: chips that only a
//!   fabric reaches (sensors, memories, expanders, power management),
//!   placed by what they are, never by which bus they speak;
//! * `modelkit/` — shared cores used from two or more domains.
//!
//! Dependencies point only downward (contract, exec, util, bus, runtime,
//! assemble); `machine` depends on this crate, never the other way.

pub mod ip;
pub mod modelkit;
pub mod part;

/// Minimal `Ctx` shared by the part-core unit tests.
#[cfg(test)]
pub(crate) mod testutil;

/// The device model catalog: every model, registered by its own module
/// (QEMU's `TypeInfo` tables, ADR 0025).
pub mod catalog;
