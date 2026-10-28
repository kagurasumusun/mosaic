//! Sensor parts: chips that measure physical quantities (temperature,
//! humidity, pressure, motion, light, cameras) plus the hardware monitors
//! (ADM/LM/MAX monitor families, ADR 0031 §3). Board wiring picks the bus
//! address; the fabric is the reuse.

use crate::catalog::ModelEntry;

pub const MODELS: &[ModelEntry] = &[];
