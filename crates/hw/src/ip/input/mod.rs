//! Input controller IP blocks (QEMU `hw/input/`): the 8042 PS/2
//! controller. Keyboards, mice and touch parts live in `crate::part::input`.
pub mod catalog;
pub mod i8042;
