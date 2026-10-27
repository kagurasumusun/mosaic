//! SD/eMMC host controllers and media: catalog entries (ADR 0025). The card
/// attaches through the host attachment (bus membership rule).
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::sd::am335x_mmchs::Am335XMmchs;
use crate::sd::bcm2835_sd::Bcm2835Sd;
use crate::sd::dwmci::Dwmci;
use crate::sd::lpc17xx_sd::Lpc17XxSd;
use crate::sd::mtk_msdc::MtkMsdc;
use crate::sd::pic32_spi_sd::Pic32SpiSd;
use crate::sd::pl181::Pl181;
use crate::sd::rk_sd::RkSd;
use crate::sd::rp2040_sd::Rp2040Sd;
use crate::sd::sdhci::Sdhci;
use crate::sd::sdhci_pci::SdhciPci;
use crate::sd::sdmmc_f1::SdmmcF1;
use crate::sd::sdmmc_f4::SdmmcF4;
use crate::sd::sdmmc_h7::SdmmcH7;
use crate::sd::sunxi_mmc::SunxiMmc;

/// SDHCI: `irq` output; the card arrives as the host attachment `host_sd()`.
fn spawn_sdhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut host = Sdhci::new(irq);
    if let Some(card) = ctx.host_sd() {
        host.attach_card(card);
    }
    machine.register_device(ctx.node, Box::new(host))?;
    Ok(())
}

/// PL181: `irq`/`fiq` outputs; the card arrives as the host attachment.
fn spawn_pl181(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let fiq = ctx.output("fiq")?;
    let mut host = Pl181::new(irq, fiq);
    if let Some(card) = ctx.host_sd() {
        host.attach_card(card);
    }
    machine.register_device(ctx.node, Box::new(host))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/pl181",
        summary: "ARM PL181 MultiMedia Card Interface (PrimeCell MCI host)",
        outputs: &[],
        spawn: spawn_pl181,
    },
    ModelEntry {
        id: "ip:sda/sdhci",
        summary: "SD Host Controller Interface v3 (SDHCI register set, PIO data port)",
        outputs: &[],
        spawn: spawn_sdhci,
    },
    ModelEntry {
        id: "ip:synopsys/dwmci",
        summary: "Synopsys DesignWare Mobile Storage Host",
        outputs: &[],
        spawn: spawn_dwmci,
    },
    ModelEntry {
        id: "ip:st/sdmmc-f1",
        summary: "STM32F1 SDIO",
        outputs: &[],
        spawn: spawn_sdmmc_f1,
    },
    ModelEntry {
        id: "ip:st/sdmmc-f4",
        summary: "STM32F4 SDIO",
        outputs: &[],
        spawn: spawn_sdmmc_f4,
    },
    ModelEntry {
        id: "ip:st/sdmmc-h7",
        summary: "STM32H7 SDMMC",
        outputs: &[],
        spawn: spawn_sdmmc_h7,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-sd",
        summary: "NXP LPC17xx SD/MMC",
        outputs: &[],
        spawn: spawn_lpc17xx_sd,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-sd",
        summary: "Raspberry Pi RP2040 SD host",
        outputs: &[],
        spawn: spawn_rp2040_sd,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-mmc",
        summary: "Allwinner Sunxi MMC",
        outputs: &[],
        spawn: spawn_sunxi_mmc,
    },
    ModelEntry {
        id: "ip:rockchip/rk-sd",
        summary: "Rockchip SDMMC",
        outputs: &[],
        spawn: spawn_rk_sd,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-msdc",
        summary: "MediaTek MSDC SD/MMC",
        outputs: &[],
        spawn: spawn_mtk_msdc,
    },
    ModelEntry {
        id: "ip:ti/am335x-mmchs",
        summary: "TI AM335x MMC/SD host",
        outputs: &[],
        spawn: spawn_am335x_mmchs,
    },
    ModelEntry {
        id: "ip:microchip/pic32-spi-sd",
        summary: "Microchip PIC32 SPI SD host",
        outputs: &[],
        spawn: spawn_pic32_spi_sd,
    },
    ModelEntry {
        id: "ip:broadcom/bcm2835-sd",
        summary: "Broadcom BCM2835 SD host",
        outputs: &[],
        spawn: spawn_bcm2835_sd,
    },
    ModelEntry {
        id: "ip:intel/sdhci-pci",
        summary: "Intel SDHCI PCI SD host",
        outputs: &[],
        spawn: spawn_sdhci_pci,
    },
];

/// Default spawn for dwmci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dwmci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dwmci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sdmmc-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sdmmc_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SdmmcF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sdmmc-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sdmmc_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SdmmcF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sdmmc-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sdmmc_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SdmmcH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-sd: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_sd(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxSd::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-sd: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_sd(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Sd::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sunxi-mmc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_mmc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiMmc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-sd: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_sd(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkSd::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-msdc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_msdc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkMsdc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-mmchs: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_mmchs(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XMmchs::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pic32-spi-sd: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_spi_sd(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32SpiSd::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for bcm2835-sd: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_bcm2835_sd(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Bcm2835Sd::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sdhci-pci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sdhci_pci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SdhciPci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
