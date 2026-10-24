#![forbid(unsafe_code)]
//! STM32F103 microcontroller slice. The unicorn feature gates the native CPU
//! backend exactly like the other board apps (ADR 0007).

#[cfg(feature = "unicorn")]
mod stm32f103;

#[cfg(feature = "unicorn")]
pub use stm32f103::{run_stm32f103, Stm32F103Report, GOLDEN_OUTPUT};
