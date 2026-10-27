//! GPIO controllers and pin multiplexers (QEMU `hw/gpio/` minus the net
//! core, which lives in `mosaic-bus::gpio`).

pub mod catalog;
pub mod gpio_f1;
pub mod pinmux;
pub mod pl061;
