//! Catalog entries for `wdt` devices (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_exec::SignalId;
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::wdt::i6300esb::I6300Esb;
use crate::wdt::iwdg_f1::IwdgF1;
use crate::wdt::iwdg_f4::IwdgF4;
use crate::wdt::iwdg_h7::IwdgH7;
use crate::wdt::lpc11xx_wwdt::Lpc11XxWwdt;
use crate::wdt::rp2040_wdt::Rp2040Wdt;
use crate::wdt::sp805_wdt::Sp805Wdt;
use crate::wdt::tco::Tco;
use crate::wdt::wwdg_f1::WwdgF1;

/// Default spawn for i6300esb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_i6300esb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = I6300Esb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for tco: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_tco(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Tco::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for sp805-wdt: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_sp805_wdt(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Sp805Wdt::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for iwdg-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_iwdg_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = IwdgF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for wwdg-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_wwdg_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = WwdgF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for iwdg-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_iwdg_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = IwdgF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for iwdg-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_iwdg_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = IwdgH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-wdt: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_wdt(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Wdt::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-wwdt: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_wwdt(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxWwdt::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:intel/i6300esb",
        summary: "Intel 6300ESB watchdog timer (PCI)",
        outputs: &[],
        spawn: spawn_i6300esb,
    },
    ModelEntry {
        id: "ip:intel/tco",
        summary: "Intel TCO watchdog timer",
        outputs: &[],
        spawn: spawn_tco,
    },
    ModelEntry {
        id: "ip:arm/sp805-wdt",
        summary: "ARM SP805 watchdog",
        outputs: &[],
        spawn: spawn_sp805_wdt,
    },
    ModelEntry {
        id: "ip:st/iwdg-f1",
        summary: "STM32F1 Independent Watchdog (IWDG)",
        outputs: &[],
        spawn: spawn_iwdg_f1,
    },
    ModelEntry {
        id: "ip:st/wwdg-f1",
        summary: "STM32F1 Window Watchdog (WWDG)",
        outputs: &[],
        spawn: spawn_wwdg_f1,
    },
    ModelEntry {
        id: "ip:st/iwdg-f4",
        summary: "STM32F4 Independent Watchdog (IWDG)",
        outputs: &[],
        spawn: spawn_iwdg_f4,
    },
    ModelEntry {
        id: "ip:st/iwdg-h7",
        summary: "STM32H7 Independent Watchdog (IWDG)",
        outputs: &[],
        spawn: spawn_iwdg_h7,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-wdt",
        summary: "Raspberry Pi RP2040 watchdog",
        outputs: &[],
        spawn: spawn_rp2040_wdt,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-wwdt",
        summary: "NXP LPC11xx Windowed Watchdog (WWDT)",
        outputs: &[],
        spawn: spawn_lpc11xx_wwdt,
    },
];
