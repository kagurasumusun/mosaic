//! IDE: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::ide::pci_ide::PciIde;
use crate::ide::piix_ide::PiixIde;
use crate::ide::ich9_ide::Ich9Ide;
use crate::ide::ahci::Ahci;
use crate::ide::jmb36x::Jmb36X;
use crate::ide::mv_sata::MvSata;
use crate::ide::vt6421::Vt6421;

fn spawn_pci_ide(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(PciIde::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/pci-ide",
    summary: "PCI IDE controller (command/block registers, two channels)",
    outputs: &[],
    spawn: spawn_pci_ide,
}];

/// Default spawn for piix-ide: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_piix_ide(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PiixIde::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ich9-ide: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ich9_ide(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ich9Ide::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ahci: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ahci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ahci::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for jmb36x: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_jmb36x(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Jmb36X::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mv-sata: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mv_sata(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MvSata::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for vt6421: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_vt6421(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Vt6421::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
