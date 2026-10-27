//! dma (QEMU `hw/dma/` role).
pub mod catalog;
pub mod i8237;

pub mod dma_330;
pub mod dma_f1;
pub mod dma_f4;
pub mod dma_h7;
pub mod ioapic_dma;
pub mod lpc11xx_dma;
pub mod lpc17xx_dma;
pub mod lpc43xx_dma;
pub mod pl080;
pub mod pl081;
pub mod rk_dma;
pub mod rp2040_dma;
pub mod sunxi_dma;
