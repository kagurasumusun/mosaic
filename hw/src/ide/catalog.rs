//! IDE: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::ide::pci_ide::PciIde;

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
