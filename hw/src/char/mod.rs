//! Serial character devices (QEMU `hw/char/` minus the UART pair fabric,
//! which lives in `mosaic-bus::uart`).

pub mod catalog;
pub mod ns16550a;
pub mod pl011;
pub mod usart_f1;
