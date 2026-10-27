//! Interrupt controllers (QEMU `hw/intc/`).

pub mod catalog;
pub mod gic_v2;
pub mod ioapic;
pub mod pic_8259;
pub mod pl190;
pub mod scs;

pub mod aia;
pub mod clint;
pub mod exti_f1;
pub mod exti_f4;
pub mod exti_h7;
pub mod gic_v3;
pub mod gic_v4;
pub mod i8259_pic;
pub mod lapic;
pub mod nvic;
pub mod nvic_f1;
pub mod pl192;
pub mod plic;
pub mod rp2040_intc;
