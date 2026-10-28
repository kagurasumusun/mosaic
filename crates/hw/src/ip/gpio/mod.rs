//! GPIO controller IP blocks and pin muxers (QEMU `hw/gpio/`): the
//! register models that drive GPIO ports. The GPIO net fabric lives in
//! `mosaic-bus::gpio`; expander chips in `crate::part::expander`.
pub mod am335x_gpio;
pub mod catalog;
pub mod efm32_gpio;
pub mod esp32_gpio;
pub mod gpio_f1;
pub mod gpio_f4;
pub mod gpio_h7;
pub mod gpio_l4;
pub mod kinetis_gpio;
pub mod lpc11xx_gpio;
pub mod lpc17xx_gpio;
pub mod mtk_gpio;
pub mod pic32_gpio;
pub mod pinmux;
pub mod pl061;
pub mod rk_gpio;
pub mod rp2040_gpio;
pub mod rp2350_gpio;
pub mod sunxi_gpio;
