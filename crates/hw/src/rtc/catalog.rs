//! Real-time clocks: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::rtc::am335x_rtc::Am335XRtc;
use crate::rtc::lpc11xx_rtc::Lpc11XxRtc;
use crate::rtc::lpc17xx_rtc::Lpc17XxRtc;
use crate::rtc::mc146818::RtcMc146818;
use crate::rtc::rp2040_rtc::Rp2040Rtc;
use crate::rtc::rtc_f1::RtcF1;
use crate::rtc::rtc_f4::RtcF4;
use crate::rtc::rtc_h7::RtcH7;
use crate::rtc::rtc_l4::RtcL4;

/// Motorola MC146818A RTC / PC CMOS: register model.
fn spawn_rtc_mc146818(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(RtcMc146818::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:motorola/mc146818",
    summary: "Motorola MC146818A RTC / PC CMOS RAM (MC146818-compatible)",
    outputs: &[],
    spawn: spawn_rtc_mc146818,
}];

/// Default spawn for rtc-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtc_f1(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RtcF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rtc-f4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtc_f4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RtcF4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rtc-h7: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtc_h7(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RtcH7::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rtc-l4: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rtc_l4(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RtcL4::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-rtc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_rtc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxRtc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc17xx-rtc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc17xx_rtc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc17XxRtc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rp2040-rtc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_rtc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Rtc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for am335x-rtc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am335x_rtc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am335XRtc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
