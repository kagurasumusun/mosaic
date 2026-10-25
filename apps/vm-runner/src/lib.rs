#![forbid(unsafe_code)]
//! Generic VM Runner supporting PC (x86_64) and Virt (ARM64) assemblies on Unicorn Engine.

#[cfg(feature = "unicorn")]
mod machine_pc;

#[cfg(feature = "unicorn")]
pub use machine_pc::{run_generic_arm64_virt_vm, run_generic_pc_vm, GenericVmReport};
