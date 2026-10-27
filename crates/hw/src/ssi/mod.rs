//! SPI controllers (QEMU `hw/ssi/` minus `core.c`, which lives in
//! `mosaic-bus::spi`).

pub mod catalog;
pub mod pl022;

pub mod am335x_spi;
pub mod efm32_usart_spi;
pub mod esp32_spi;
pub mod lpc11xx_spi;
pub mod lpc17xx_spi;
pub mod lpc43xx_spi;
pub mod mtk_spi;
pub mod pic32_spi;
pub mod rk_spi;
pub mod rp2040_spi;
pub mod spi_f1;
pub mod spi_f4;
pub mod spi_h7;
pub mod spi_l4;
pub mod sunxi_spi;
