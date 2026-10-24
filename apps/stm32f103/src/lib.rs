#![forbid(unsafe_code)]
//! STM32F103 microcontroller slice. The unicorn feature gates the native CPU
//! backend exactly like the other board apps (ADR 0007). The machine itself is
//! built from the catalog declaration with or without that backend.

#[cfg(any(test, feature = "unicorn", feature = "native"))]
mod board;
#[cfg(any(test, feature = "unicorn", feature = "native"))]
mod manifest;

#[cfg(any(test, feature = "unicorn", feature = "native"))]
mod image;

#[cfg(feature = "native")]
mod native;
#[cfg(feature = "unicorn")]
mod stm32f103;

#[cfg(all(feature = "native", not(feature = "unicorn")))]
pub use image::{Stm32F103Report, GOLDEN_OUTPUT};
#[cfg(feature = "native")]
pub use native::run_stm32f103_native;
#[cfg(feature = "unicorn")]
pub use stm32f103::{run_stm32f103, Stm32F103Report, GOLDEN_OUTPUT};
