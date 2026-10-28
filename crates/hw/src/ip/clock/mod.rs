//! Clock generation, gating and reset IP blocks: the RCC family,
//! CGUs and clock managers (ADR 0031 §4).
pub mod catalog;
pub mod cm_am335x;
pub mod lpc11xx_cgu;
pub mod rcc_f1;
pub mod rcc_f4;
pub mod rcc_h7;
pub mod rp2040_clocks;
