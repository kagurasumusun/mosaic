//! Mailboxes: catalog entries (ADR 0025).

use crate::mailbox::mhu::UnmappedPolicy;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::mailbox::mhu::{Mhu, MhuParams};

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
