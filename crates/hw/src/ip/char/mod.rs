//! Serial character device controllers (QEMU `hw/char/`): the MMIO and
//! port-IO UART/modem register models. The UART line fabric lives in
//! `mosaic-bus::uart`; devices behind it live in `crate::part::input`.
pub mod adsp_sc5xx_uart;
pub mod am335x_uart;
pub mod catalog;
pub mod efm32_leuart;
pub mod efm32_usart;
pub mod efr32_usart;
pub mod esp32_uart;
pub mod esp32s3_uart;
pub mod kinetis_lpuart;
pub mod kinetis_uart;
pub mod lpc11xx_uart;
pub mod lpc17xx_uart;
pub mod lpc43xx_uart;
pub mod lpuart_h7;
pub mod lpuart_l4;
pub mod lx6_uart;
pub mod max32660_uart;
pub mod mtk_uart;
pub mod ns16550a;
pub mod omap_uart;
pub mod pic32_uart;
pub mod pl011;
pub mod ra6_uart;
pub mod rk_uart;
pub mod rp2040_uart;
pub mod rp2350_uart;
pub mod rx_uart;
pub mod rza1_uart;
pub mod sunxi_uart;
pub mod uart_l4;
pub mod usart_f1;
pub mod usart_f4;
pub mod usart_f7;
pub mod usart_h7;
