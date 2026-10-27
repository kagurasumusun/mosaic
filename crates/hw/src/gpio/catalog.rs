//! GPIO controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::gpio::am335x_gpio::Am335XGpio;
use crate::gpio::efm32_gpio::Efm32Gpio;
use crate::gpio::esp32_gpio::Esp32Gpio;
use crate::gpio::gpio_f1::{GpioF1, GpioF1Params};
use crate::gpio::gpio_f4::GpioF4;
use crate::gpio::gpio_h7::GpioH7;
use crate::gpio::gpio_l4::GpioL4;
use crate::gpio::kinetis_gpio::KinetisGpio;
use crate::gpio::lpc11xx_gpio::Lpc11XxGpio;
use crate::gpio::lpc17xx_gpio::Lpc17XxGpio;
use crate::gpio::mtk_gpio::MtkGpio;
use crate::gpio::pic32_gpio::Pic32Gpio;
use crate::gpio::pl061::{Pl061, Pl061Params};
use crate::gpio::rk_gpio::RkGpio;
use crate::gpio::rp2040_gpio::Rp2040Gpio;
use crate::gpio::rp2350_gpio::Rp2350Gpio;
use crate::gpio::sunxi_gpio::SunxiGpio;

/// STM32F1 GPIO port: `clock` input from the RCC gate.
fn spawn_gpio_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut gpio = GpioF1::new(GpioF1Params);
    if let Some(wire) = ctx.incoming.iter().find(|wire| wire.port == "clock") {
        gpio.bind_clock(wire.source, wire.signal);
    }
    machine.register_device(ctx.node, Box::new(gpio))?;
    Ok(())
}

/// ARM PrimeCell GPIO (PL061): `combined` IRQ output.
fn spawn_pl061(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("combined")?;
    let gpio = Pl061::new(Pl061Params::new(irq));
    machine.register_device(ctx.node, Box::new(gpio))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:st/gpio-f1",
        summary: "STM32F1 GPIO port (RM0008) with clock gate and pin signals",
        outputs: &[],
        spawn: spawn_gpio_f1,
    },
    ModelEntry {
        id: "ip:arm/pl061",
        summary: "ARM PrimeCell GPIO (PL061) functional register model",
        outputs: &[],
        spawn: spawn_pl061,
    },
    ModelEntry {
        id: "ip:st/gpio-f4",
        summary: "STM32F4 GPIO (RM0090)",
        outputs: &[],
        spawn: spawn_gpio_f4,
    },
    ModelEntry {
        id: "ip:st/gpio-h7",
        summary: "STM32H7 GPIO",
        outputs: &[],
        spawn: spawn_gpio_h7,
    },
    ModelEntry {
        id: "ip:st/gpio-l4",
        summary: "STM32L4 GPIO",
        outputs: &[],
        spawn: spawn_gpio_l4,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-gpio",
        summary: "NXP LPC11xx GPIO",
        outputs: &[],
        spawn: spawn_lpc11xx_gpio,
    },
    ModelEntry {
        id: "ip:nxp/lpc17xx-gpio",
        summary: "NXP LPC17xx GPIO",
        outputs: &[],
        spawn: spawn_lpc17xx_gpio,
    },
    ModelEntry {
        id: "ip:nxp/kinetis-gpio",
        summary: "NXP Kinetis GPIO",
        outputs: &[],
        spawn: spawn_kinetis_gpio,
    },
    ModelEntry {
        id: "ip:microchip/pic32-gpio",
        summary: "Microchip PIC32 GPIO",
        outputs: &[],
        spawn: spawn_pic32_gpio,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-gpio",
        summary: "Raspberry Pi RP2040 GPIO (SIO)",
        outputs: &[],
        spawn: spawn_rp2040_gpio,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2350-gpio",
        summary: "Raspberry Pi RP2350 GPIO (SIO)",
        outputs: &[],
        spawn: spawn_rp2350_gpio,
    },
    ModelEntry {
        id: "ip:allwinner/sunxi-gpio",
        summary: "Allwinner Sunxi GPIO",
        outputs: &[],
        spawn: spawn_sunxi_gpio,
    },
    ModelEntry {
        id: "ip:rockchip/rk-gpio",
        summary: "Rockchip GPIO",
        outputs: &[],
        spawn: spawn_rk_gpio,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-gpio",
        summary: "MediaTek GPIO",
        outputs: &[],
        spawn: spawn_mtk_gpio,
    },
    ModelEntry {
        id: "ip:ti/am335x-gpio",
        summary: "TI AM335x GPIO",
        outputs: &[],
        spawn: spawn_am335x_gpio,
    },
    ModelEntry {
        id: "ip:espressif/esp32-gpio",
        summary: "Espressif ESP32 GPIO",
        outputs: &[],
        spawn: spawn_esp32_gpio,
    },
    ModelEntry {
        id: "ip:silabs/efm32-gpio",
        summary: "Silicon Labs EFM32 GPIO",
        outputs: &[],
        spawn: spawn_efm32_gpio,
    },
];

/// Default spawn for gpio-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gpio_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = GpioF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for gpio-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gpio_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = GpioH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for gpio-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_gpio_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = GpioL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for kinetis-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_kinetis_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = KinetisGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pic32-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pic32_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pic32Gpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Gpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2350-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2350_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2350Gpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sunxi-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sunxi_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = SunxiGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XGpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for esp32-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_esp32_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Esp32Gpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for efm32-gpio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_efm32_gpio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Efm32Gpio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
