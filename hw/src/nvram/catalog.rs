//! Firmware configuration interfaces: catalog entries (ADR 2025 corrected:
//! ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::nvram::fw_cfg::FwCfg;

/// QEMU fw_cfg: register model.
fn spawn_fw_cfg(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(FwCfg::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[ModelEntry {
    id: "ip:x86/fw-cfg",
    summary: "QEMU fw_cfg firmware configuration interface (DMA-less register model)",
    outputs: &[],
    spawn: spawn_fw_cfg,
}];
