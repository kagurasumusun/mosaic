//! Interrupt controllers (QEMU `hw/intc/`).

pub mod catalog;
pub mod gic_v2;
pub mod ioapic;
pub mod pic_8259;
pub mod pl190;
pub mod scs;
