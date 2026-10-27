//! Real-time clocks: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::rtc::mc146818::RtcMc146818;

/// Motorola MC146818A RTC / PC CMOS: register model.
fn spawn_rtc_mc146818(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(RtcMc146818::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:motorola/mc146818",
    summary: "Motorola MC146818A RTC / PC CMOS RAM (MC146818-compatible)",
    outputs: &[],
    spawn: spawn_rtc_mc146818,
}];
