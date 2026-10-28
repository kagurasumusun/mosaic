//! The model author's toolkit (ADR 0031 §6): cross-domain shared cores
//! promoted by the rule of two. Domain-local family cores stay in their
//! domain directories; only cores used from two or more domains live here.
pub mod i2c_pointer;
pub mod primecell_id;
pub mod pwm_device_core;
pub mod unmapped_policy;
