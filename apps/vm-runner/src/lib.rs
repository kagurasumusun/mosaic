#![forbid(unsafe_code)]
//! Generic VM runner: PC (x86/x86_64) and virt (ARM64) assemblies on Unicorn.
//!
//! `machine_pc` is the device-enumeration slice (PCI/IDE/virtio discovery).
//! `pc_boot` is the real bring-up slice: it answers the firmware software
//! interrupts a bootloader calls and executes the guest's own real-mode code.

#[cfg(feature = "unicorn")]
mod machine_pc;

#[cfg(feature = "unicorn")]
pub mod linux_boot;
pub mod pc_boot;

#[cfg(feature = "unicorn")]
pub use machine_pc::{run_generic_arm64_virt_vm, run_generic_pc_vm, GenericVmReport};

#[cfg(feature = "unicorn")]
pub use pc_boot::{
    el_torito_image, run_pc_boot, BootPoint, DiskImage, ElToritoImage, PcBootOptions, PcBootReport,
};
