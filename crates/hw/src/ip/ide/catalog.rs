//! IDE/SATA controller IP blocks (QEMU `hw/ide/`).

use crate::catalog::ModelEntry;
use crate::ip::ide::ahci::Ahci;
use crate::ip::ide::ich9_ide::Ich9Ide;
use crate::ip::ide::jmb36x::Jmb36X;
use crate::ip::ide::mv_sata::MvSata;
use crate::ip::ide::pci_ide::PciIde;
use crate::ip::ide::piix_ide::PiixIde;
use crate::ip::ide::vt6421::Vt6421;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

fn spawn_pci_ide(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(PciIde::new()))?;
    Ok(())
}
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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:pci/pci-ide",
        summary: "PCI IDE controller (command/block registers, two channels)",
        outputs: &[],
        spawn: spawn_pci_ide,
    },
    ModelEntry {
        id: "ip:intel/ahci",
        summary: "Intel AHCI SATA controller",
        outputs: &[],
        spawn: spawn_ahci,
    },
    ModelEntry {
        id: "ip:jmicron/jmb36x",
        summary: "JMicron JMB36x SATA/IDE controller",
        outputs: &[],
        spawn: spawn_jmb36x,
    },
    ModelEntry {
        id: "ip:via/vt6421",
        summary: "VIA VT6421 SATA controller",
        outputs: &[],
        spawn: spawn_vt6421,
    },
    ModelEntry {
        id: "ip:intel/piix-ide",
        summary: "Intel PIIX3/PIIX4 IDE controller",
        outputs: &[],
        spawn: spawn_piix_ide,
    },
    ModelEntry {
        id: "ip:intel/ich9-ide",
        summary: "Intel ICH9 SATA IDE controller",
        outputs: &[],
        spawn: spawn_ich9_ide,
    },
    ModelEntry {
        id: "ip:marvell/mv-sata",
        summary: "Marvell SATA controller",
        outputs: &[],
        spawn: spawn_mv_sata,
    },
];
