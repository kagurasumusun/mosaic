//! I2C controllers and switches (QEMU `hw/i2c/` minus `core.c`, which
//! lives in `mosaic-bus::i2c`).

pub mod catalog;
pub mod dw_apb_i2c;
pub mod tca9548a;
