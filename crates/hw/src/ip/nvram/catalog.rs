//! Firmware-facing NVRAM IP blocks (QEMU `hw/nvram/`); NVRAM chips
//! live in `crate::part::memory`.

use crate::catalog::ModelEntry;
use crate::ip::nvram::fw_cfg::FwCfg;
use crate::ip::nvram::lpc11xx_flash::Lpc11XxFlash;
use crate::ip::nvram::option_bytes_f1::OptionBytesF1;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// QEMU fw_cfg: register model.
fn spawn_fw_cfg(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(FwCfg::new()))?;
    Ok(())
}
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
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
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
