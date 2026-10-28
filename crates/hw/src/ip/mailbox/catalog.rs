//! Inter-processor mailbox IP blocks (QEMU `hw/mailbox/`).

use crate::catalog::ModelEntry;
use crate::ip::mailbox::am654_mailbox::Am654Mailbox;
use crate::ip::mailbox::arm_smc_mailbox::ArmSmcMailbox;
use crate::ip::mailbox::mhu::UnmappedPolicy;
use crate::ip::mailbox::mhu::{Mhu, MhuParams};
use crate::ip::mailbox::rp2040_sio::Rp2040Sio;
use crate::ip::mailbox::smc::Smc;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// ARM MHU: register model.
fn spawn_mhu(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(
        ctx.node,
        Box::new(Mhu::new(MhuParams::new(UnmappedPolicy::RazWi))),
    )?;
    Ok(())
}
/// Default spawn for smc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_smc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Smc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for rp2040-sio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rp2040_sio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Rp2040Sio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for am654-mailbox: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_am654_mailbox(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am654Mailbox::new(irq);
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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:arm/mhu",
        summary: "ARM MHU mailbox (doorbell + combined + channel registers)",
        outputs: &[],
        spawn: spawn_mhu,
    },
    ModelEntry {
        id: "ip:arm/smc",
        summary: "ARM SMC mailbox",
        outputs: &[],
        spawn: spawn_smc,
    },
    ModelEntry {
        id: "ip:raspberrypi/rp2040-sio",
        summary: "Raspberry Pi RP2040 SIO mailbox",
        outputs: &[],
        spawn: spawn_rp2040_sio,
    },
    ModelEntry {
        id: "ip:ti/am654-mailbox",
        summary: "TI AM654 Secure Mailbox",
        outputs: &[],
        spawn: spawn_am654_mailbox,
    },
    ModelEntry {
        id: "ip:arm/arm-smc-mailbox",
        summary: "ARM Secure Monitor Call (SMC) mailbox",
        outputs: &[],
        spawn: spawn_arm_smc_mailbox,
    },
];
