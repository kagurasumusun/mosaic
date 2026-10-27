//! USB host controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::usb::uhci::Uhci;

/// Intel UHCI: `irq` output; the USB fabric (with its devices) arrives as
/// the host attachment `host_usb()`.
fn spawn_uhci(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let mut controller = Uhci::new(irq);
    if let Some(bus) = ctx.host_usb() {
        controller.attach_bus(bus);
    }
    machine.register_device(ctx.node, Box::new(controller))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/uhci",
    summary: "Intel UHCI USB 1.1 host controller (frame list, TD walk, PCI function)",
    outputs: &[],
    spawn: spawn_uhci,
}];
