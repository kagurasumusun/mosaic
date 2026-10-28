//! I3C bus devices: the generic MIPI I3C Basic target and future
//! I3C-attached parts.

use crate::catalog::ModelEntry;
use crate::part::i3c::i3c_target::I3cTargetModel;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_contract::i3c::I3cAddr;
use mosaic_runtime::Machine;

/// with a manifest-declared PID is spawnable: it carries no wiring beyond
/// its identity.
fn spawn_i3c_target(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let pid = u64::from_str_radix(
        ctx.parameter_str("pid")
            .unwrap_or("0xA00000000001")
            .trim_start_matches("0x"),
        16,
    )
    .unwrap_or(0xA000_0000_0001);
    let static_address = ctx
        .parameter_int("static-address")
        .map(|value| I3cAddr(value as u8));
    machine.register_device(ctx.node, Box::new(I3cTargetModel::new(pid, static_address)))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "part:mipi/i3c-target",
    summary: "Generic MIPI I3C Basic-spec target (DAA, CCC, SDR register bank)",
    outputs: &[],
    spawn: spawn_i3c_target,
}];
