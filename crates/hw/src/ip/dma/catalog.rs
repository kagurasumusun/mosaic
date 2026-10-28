//! DMA controller IP blocks (QEMU `hw/dma/`).

use crate::catalog::ModelEntry;
use crate::ip::dma::dma_330::Dma330;
use crate::ip::dma::dma_f1::DmaF1;
use crate::ip::dma::dma_f4::DmaF4;
use crate::ip::dma::dma_h7::DmaH7;
use crate::ip::dma::i8237::I8237;
use crate::ip::dma::ioapic_dma::IoapicDma;
use crate::ip::dma::lpc11xx_dma::Lpc11XxDma;
use crate::ip::dma::lpc17xx_dma::Lpc17XxDma;
use crate::ip::dma::lpc43xx_dma::Lpc43XxDma;
use crate::ip::dma::pl080::Pl080;
use crate::ip::dma::pl081::Pl081;
use crate::ip::dma::rk_dma::RkDma;
use crate::ip::dma::rp2040_dma::Rp2040Dma;
use crate::ip::dma::sunxi_dma::SunxiDma;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// with a manifest-declared PID is spawnable: it carries no wiring beyond
/// its identity.
/// Intel 8237A: no IRQ outputs on the node (DRQ inputs arrive as signal
/// edges; DACK outputs through the wiring the board declares).
fn spawn_i8237(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(I8237::new()))?;
    Ok(())
}
/// Default spawn for ioapic-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ioapic_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = IoapicDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pl080: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pl080(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pl080::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pl081: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pl081(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pl081::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dma-330: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dma_330(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Dma330::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dma-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dma_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = DmaF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dma-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dma_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = DmaF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for dma-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_dma_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = DmaH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc11xx-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc17xx-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for lpc43xx-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc43xx_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc43XxDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2040-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Dma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for sunxi-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rk-dma: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_dma(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkDma::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:intel/i8237",
        summary: "Intel 8237A DMA controller, cascaded master+slave (PC/AT 8 channels)",
        outputs: &[],
        spawn: spawn_i8237,
    },
    ModelEntry {
        id: "ip:arm/pl080",
        summary: "ARM PL080 DMA controller",
        outputs: &[],
        spawn: spawn_pl080,
    },
    ModelEntry {
        id: "ip:arm/pl081",
        summary: "ARM PL081 DMA controller",
        outputs: &[],
        spawn: spawn_pl081,
    },
    ModelEntry {
        id: "ip:intel/ioapic-dma",
        summary: "Intel I/O APIC DMA extension",
        outputs: &[],
        spawn: spawn_ioapic_dma,
    },
    ModelEntry {
        id: "ip:arm/dma-330",
        summary: "ARM DMA-330 DMA controller",
        outputs: &[],
        spawn: spawn_dma_330,
    },
    ModelEntry {
        id: "ip:st/dma-f1",
        summary: "STM32F1 DMA",
        outputs: &[],
        spawn: spawn_dma_f1,
    },
    ModelEntry {
        id: "ip:st/dma-f4",
        summary: "STM32F4 DMA",
        outputs: &[],
        spawn: spawn_dma_f4,
    },
    ModelEntry {
        id: "ip:st/dma-h7",
        summary: "STM32H7 DMA",
        outputs: &[],
        spawn: spawn_dma_h7,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-dma",
        summary: "NXP LPC11xx GPDMA",
        outputs: &[],
        spawn: spawn_lpc11xx_dma,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-dma",
        summary: "NXP LPC17xx GPDMA",
        outputs: &[],
        spawn: spawn_lpc17xx_dma,
    },
    ModelEntry {
        id: "ip:nxp/lpc43xx-dma",
        summary: "NXP LPC43xx SCT/DMA",
        outputs: &[],
        spawn: spawn_lpc43xx_dma,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-dma",
        summary: "Raspberry Pi RP2040 DMA",
        outputs: &[],
        spawn: spawn_rp2040_dma,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-dma",
        summary: "Allwinner Sunxi DMA",
        outputs: &[],
        spawn: spawn_sunxi_dma,
    },
    ModelEntry {
        id: "ip:rockchip/rk-dma",
        summary: "Rockchip DMA",
        outputs: &[],
        spawn: spawn_rk_dma,
    },
];
