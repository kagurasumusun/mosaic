//! Mailboxes: catalog entries (ADR 0025).

use crate::mailbox::mhu::UnmappedPolicy;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::mailbox::mhu::{Mhu, MhuParams};
use crate::mailbox::smc::Smc;
use crate::mailbox::rp2040_sio::Rp2040Sio;
use crate::mailbox::am654_mailbox::Am654Mailbox;

/// ARM MHU: register model.
fn spawn_mhu(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(
        ctx.node,
        Box::new(Mhu::new(MhuParams::new(UnmappedPolicy::RazWi))),
    )?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:arm/mhu",
    summary: "ARM MHU mailbox (doorbell + combined + channel registers)",
    outputs: &[],
    spawn: spawn_mhu,
}];

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
fn spawn_am654_mailbox(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Am654Mailbox::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
