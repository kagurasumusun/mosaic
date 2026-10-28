//! Clock generation, gating and reset IP blocks: the RCC family,
//! CGUs and clock managers (ADR 0031 §4).

use crate::catalog::ModelEntry;
use crate::ip::clock::cm_am335x::CmAm335X;
use crate::ip::clock::lpc11xx_cgu::Lpc11XxCgu;
use crate::ip::clock::rcc_f1::{self, RccF1, RccF1Params};
use crate::ip::clock::rcc_f4::RccF4;
use crate::ip::clock::rcc_h7::RccH7;
use crate::ip::clock::rp2040_clocks::Rp2040Clocks;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:st/rcc-f1",
        summary: "STM32F1 reset and clock control (RM0008): clock tree, CIR, 43 gate outputs",
        outputs: RCC_F1_GATES,
        spawn: spawn_rcc_f1,
    },
    ModelEntry {
        id: "ip:st/rcc-f4",
        summary: "STM32F4 Reset and Clock Control (RCC)",
        outputs: &[],
        spawn: spawn_rcc_f4,
    },
    ModelEntry {
        id: "ip:st/rcc-h7",
        summary: "STM32H7 Reset and Clock Control (RCC)",
        outputs: &[],
        spawn: spawn_rcc_h7,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-cgu",
        summary: "NXP LPC11xx Clock Generation Unit",
        outputs: &[],
        spawn: spawn_lpc11xx_cgu,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-clocks",
        summary: "Raspberry Pi RP2040 clock controller",
        outputs: &[],
        spawn: spawn_rp2040_clocks,
    },
    ModelEntry {
        id: "ip:ti/cm-am335x",
        summary: "TI AM335x Clock Manager (CM)",
        outputs: &[],
        spawn: spawn_cm_am335x,
    },
];
