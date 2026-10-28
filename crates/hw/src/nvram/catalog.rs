//! Firmware configuration interfaces: catalog entries (ADR 2025 corrected:
//! ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::nvram::fw_cfg::FwCfg;
use crate::nvram::lpc11xx_flash::Lpc11XxFlash;
use crate::nvram::option_bytes_f1::OptionBytesF1;

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
},
    ModelEntry {
        id: "ip:st/option-bytes-f1",
        summary: "STM32F1 option bytes (NVRAM)",
        outputs: &[],
        spawn: spawn_option_bytes_f1,
    },
    ModelEntry {
        id: "ip:nxp/lpc11xx-flash",
        summary: "NXP LPC11xx flash controller",
        outputs: &[],
        spawn: spawn_lpc11xx_flash,
    },
];

/// Default spawn for option-bytes-f1: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_option_bytes_f1(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = OptionBytesF1::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for lpc11xx-flash: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_lpc11xx_flash(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Lpc11XxFlash::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
