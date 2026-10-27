//! Miscellaneous devices: catalog entries (ADR 0025). The RCC publishes its
//! whole gate table here — `clk` edges resolve their source signal through
//! it, so a board wires `rcc.gate-apb2-iopa` in its manifest and nothing else.

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::misc::arm_smc_mailbox::ArmSmcMailbox;
use crate::misc::ast_lpc::AstLpc;
use crate::misc::cm_am335x::CmAm335X;
use crate::misc::csme::Csme;
use crate::misc::ish::Ish;
use crate::misc::lpc11xx_cgu::Lpc11XxCgu;
use crate::misc::npcm_lpc::NpcmLpc;
use crate::misc::p2sb::P2Sb;
use crate::misc::pc_testdev::PcTestdev;
use crate::misc::pcspk::Pcspk;
use crate::misc::pmc::Pmc;
use crate::misc::pmt::Pmt;
use crate::misc::rcc_f1::{self, RccF1, RccF1Params};
use crate::misc::rcc_f4::RccF4;
use crate::misc::rcc_h7::RccH7;
use crate::misc::rp2040_clocks::Rp2040Clocks;

/// STM32F1 RCC: `rcc` IRQ output (CIR, NVIC position 5), every AHB/APB1/APB2
/// clock gate as a `gate-*` output port, `hse-hz`/`hsi-hz` parameters.
fn spawn_rcc_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("rcc")?;
    let mut params = RccF1Params::new(irq);
    if let Some(hse_hz) = ctx.parameter_int("hse-hz") {
        if let Ok(hse_hz) = u32::try_from(hse_hz) {
            params.hse_hz = hse_hz;
        }
    }
    if let Some(hsi_hz) = ctx.parameter_int("hsi-hz") {
        if let Ok(hsi_hz) = u32::try_from(hsi_hz) {
            params.hsi_hz = hsi_hz;
        }
    }
    machine.register_device(ctx.node, Box::new(RccF1::new(params)))?;
    Ok(())
}

/// The RCC's vendor gate table: manifest `clk` port name -> model-internal
/// SignalId (RM0008 AHBENR/APB1ENR/APB2ENR gates, ADR 0008).
pub const RCC_F1_GATES: &[(&str, mosaic_exec::SignalId)] = &[
    ("gate-ahb-dma1", rcc_f1::GATE_AHB_DMA1),
    ("gate-ahb-dma2", rcc_f1::GATE_AHB_DMA2),
    ("gate-ahb-sram", rcc_f1::GATE_AHB_SRAM),
    ("gate-ahb-flitf", rcc_f1::GATE_AHB_FLITF),
    ("gate-ahb-crc", rcc_f1::GATE_AHB_CRC),
    ("gate-apb1-tim2", rcc_f1::GATE_APB1_TIM2),
    ("gate-apb1-tim3", rcc_f1::GATE_APB1_TIM3),
    ("gate-apb1-tim4", rcc_f1::GATE_APB1_TIM4),
    ("gate-apb1-tim5", rcc_f1::GATE_APB1_TIM5),
    ("gate-apb1-tim6", rcc_f1::GATE_APB1_TIM6),
    ("gate-apb1-tim7", rcc_f1::GATE_APB1_TIM7),
    ("gate-apb1-tim12", rcc_f1::GATE_APB1_TIM12),
    ("gate-apb1-tim13", rcc_f1::GATE_APB1_TIM13),
    ("gate-apb1-tim14", rcc_f1::GATE_APB1_TIM14),
    ("gate-apb1-wwdg", rcc_f1::GATE_APB1_WWDG),
    ("gate-apb1-spi2", rcc_f1::GATE_APB1_SPI2),
    ("gate-apb1-spi3", rcc_f1::GATE_APB1_SPI3),
    ("gate-apb1-usart2", rcc_f1::GATE_APB1_USART2),
    ("gate-apb1-usart3", rcc_f1::GATE_APB1_USART3),
    ("gate-apb1-uart4", rcc_f1::GATE_APB1_UART4),
    ("gate-apb1-uart5", rcc_f1::GATE_APB1_UART5),
    ("gate-apb1-i2c1", rcc_f1::GATE_APB1_I2C1),
    ("gate-apb1-i2c2", rcc_f1::GATE_APB1_I2C2),
    ("gate-apb1-usb", rcc_f1::GATE_APB1_USB),
    ("gate-apb1-can1", rcc_f1::GATE_APB1_CAN1),
    ("gate-apb1-bkp", rcc_f1::GATE_APB1_BKP),
    ("gate-apb1-pwr", rcc_f1::GATE_APB1_PWR),
    ("gate-apb1-dac", rcc_f1::GATE_APB1_DAC),
    ("gate-apb2-afio", rcc_f1::GATE_APB2_AFIO),
    ("gate-apb2-iopa", rcc_f1::GATE_APB2_IOPA),
    ("gate-apb2-iopb", rcc_f1::GATE_APB2_IOPB),
    ("gate-apb2-iopc", rcc_f1::GATE_APB2_IOPC),
    ("gate-apb2-iopd", rcc_f1::GATE_APB2_IOPD),
    ("gate-apb2-iope", rcc_f1::GATE_APB2_IOPE),
    ("gate-apb2-iopf", rcc_f1::GATE_APB2_IOPF),
    ("gate-apb2-iopg", rcc_f1::GATE_APB2_IOPG),
    ("gate-apb2-adc1", rcc_f1::GATE_APB2_ADC1),
    ("gate-apb2-adc2", rcc_f1::GATE_APB2_ADC2),
    ("gate-apb2-tim1", rcc_f1::GATE_APB2_TIM1),
    ("gate-apb2-spi1", rcc_f1::GATE_APB2_SPI1),
    ("gate-apb2-tim8", rcc_f1::GATE_APB2_TIM8),
    ("gate-apb2-usart1", rcc_f1::GATE_APB2_USART1),
];

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:st/rcc-f1",
    summary: "STM32F1 reset and clock control (RM0008): clock tree, CIR, 43 gate outputs",
    outputs: RCC_F1_GATES,
    spawn: spawn_rcc_f1,
}];

/// Default spawn for rcc-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rcc_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RccF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rcc-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rcc_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RccH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-cgu: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_cgu(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxCgu::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-clocks: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_clocks(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Clocks::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for cm-am335x: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_cm_am335x(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = CmAm335X::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pcspk: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pcspk(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pcspk::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pc-testdev: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pc_testdev(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PcTestdev::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pmc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pmc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pmc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pmt: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pmt(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pmt::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for p2sb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_p2sb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = P2Sb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ast-lpc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ast_lpc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = AstLpc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for npcm-lpc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_npcm_lpc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = NpcmLpc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ish: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ish(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ish::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for csme: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_csme(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Csme::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for arm-smc-mailbox: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_arm_smc_mailbox(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = ArmSmcMailbox::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
