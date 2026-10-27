//! sd (QEMU `hw/sd/` role).
pub mod catalog;
pub mod pl181;
pub mod sd_card;
pub mod sdhci;

pub mod am335x_mmchs;
pub mod bcm2835_sd;
pub mod dwmci;
pub mod lpc17xx_sd;
pub mod mtk_msdc;
pub mod pic32_spi_sd;
pub mod rk_sd;
pub mod rp2040_sd;
pub mod sdhci_pci;
pub mod sdmmc_f1;
pub mod sdmmc_f4;
pub mod sdmmc_h7;
pub mod sunxi_mmc;
