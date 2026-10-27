//! Timers (QEMU `hw/timer/`).

pub mod catalog;
pub mod hpet;
pub mod pit_8254;
pub mod sp804;

pub mod am335x_timer;
pub mod efm32_timer;
pub mod i8253_pit;
pub mod i8254_pit;
pub mod i8259_pit;
pub mod kinetis_ftm;
pub mod kinetis_lptmr;
pub mod kinetis_pit;
pub mod lpc11xx_timer;
pub mod lpc17xx_timer;
pub mod mtk_timer;
pub mod pic32_timer;
pub mod rk_timer;
pub mod rp2040_pwm;
pub mod rp2040_timer;
pub mod rza1_timer;
pub mod sp805;
pub mod sunxi_timer;
pub mod tim_f1;
pub mod tim_f4;
pub mod tim_h7;
pub mod tim_l4;
