//! Input controller IP blocks (QEMU `hw/input/`): the 8042 PS/2
//! controller. Keyboards, mice and touch parts live in `crate::part::input`.

use crate::catalog::ModelEntry;
use crate::ip::input::i8042::I8042;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// The 8042 keyboard controller: `irq` output. The keyboard and mouse
/// devices attach through the board's wiring (bus membership rule).
fn spawn_i8042(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    machine.register_device(ctx.node, Box::new(I8042::new(irq)))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/i8042",
    summary: "Intel 8042 PS/2 keyboard/mouse controller (A20 gate, reset line)",
    outputs: &[],
    spawn: spawn_i8042,
}];
