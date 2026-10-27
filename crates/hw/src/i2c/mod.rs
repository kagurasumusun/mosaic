//! I2C controllers and switches (QEMU `hw/i2c/` minus `core.c`, which
//! lives in `mosaic-bus::i2c`).

pub mod catalog;
pub mod dw_apb_i2c;
pub mod tca9548a;

pub mod am335x_i2c;
pub mod efm32_i2c;
pub mod esp32_i2c;
pub mod i2c_f1;
pub mod i2c_f4;
pub mod i2c_f7;
pub mod i2c_h7;
pub mod i2c_l4;
pub mod lpc11xx_i2c;
pub mod lpc17xx_i2c;
pub mod lpc43xx_i2c;
pub mod mtk_i2c;
pub mod omap_i2c;
pub mod pic32_i2c;
pub mod rk_i2c;
pub mod rp2040_i2c;
pub mod sunxi_i2c;
