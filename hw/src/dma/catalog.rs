//! DMA controllers: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::dma::i8237::I8237;

/// Intel 8237A: no IRQ outputs on the node (DRQ inputs arrive as signal
/// edges; DACK outputs through the wiring the board declares).
fn spawn_i8237(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(I8237::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:intel/i8237",
    summary: "Intel 8237A DMA controller, cascaded master+slave (PC/AT 8 channels)",
    outputs: &[],
    spawn: spawn_i8237,
}];
