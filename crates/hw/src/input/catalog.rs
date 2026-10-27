//! Input devices: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::input::i8042::I8042;
use crate::input::ps2_mouse::Ps2Mouse;
use crate::input::ps2_touchpad::Ps2Touchpad;

/// The 8042 keyboard controller: `irq` output. The keyboard and mouse
/// devices attach through the board's wiring (bus membership rule).
fn spawn_i8042(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    machine.register_device(ctx.node, Box::new(I8042::new(irq)))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/8042",
    summary: "Intel 8042 PS/2 keyboard/mouse controller (A20 gate, reset line)",
    outputs: &[],
    spawn: spawn_i8042,
}];

/// Default spawn for ps2-touchpad: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ps2_touchpad(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ps2Touchpad::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ps2-mouse: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ps2_mouse(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ps2Mouse::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
