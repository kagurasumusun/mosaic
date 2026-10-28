//! Input parts: PS/2 keyboards and mice, touch controllers, USB HID
//! devices, video capture — and the PS/2 family cores.

use crate::catalog::ModelEntry;
use crate::part::input::ps2_mouse::Ps2Mouse;
use crate::part::input::ps2_touchpad::Ps2Touchpad;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "part:synaptics/ps2-touchpad",
        summary: "Synaptics PS/2 touchpad",
        outputs: &[],
        spawn: spawn_ps2_touchpad,
    },
    ModelEntry {
        id: "part:logitech/ps2-mouse",
        summary: "Logitech PS/2 mouse",
        outputs: &[],
        spawn: spawn_ps2_mouse,
    },
];
