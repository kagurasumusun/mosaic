//! CPU-addressed platform glue (ADR 0031 §4): PCH service blocks,
//! PC speaker, the QEMU test device, PrimeCell ID registers. External
//! EC/SIO/BMC chips live in `crate::part::ec`.

use crate::catalog::ModelEntry;
use crate::ip::platform::ast_lpc::AstLpc;
use crate::ip::platform::csme::Csme;
use crate::ip::platform::ish::Ish;
use crate::ip::platform::npcm_lpc::NpcmLpc;
use crate::ip::platform::p2sb::P2Sb;
use crate::ip::platform::pc_testdev::PcTestdev;
use crate::ip::platform::pcspk::Pcspk;
use crate::ip::platform::pmc::Pmc;
use crate::ip::platform::pmt::Pmt;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// Default spawn for pcspk: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pcspk(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pcspk::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pc-testdev: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pc_testdev(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PcTestdev::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pmc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pmc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pmc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for pmt: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pmt(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Pmt::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for p2sb: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_p2sb(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = P2Sb::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ast-lpc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ast_lpc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = AstLpc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for npcm-lpc: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_npcm_lpc(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = NpcmLpc::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for ish: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ish(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Ish::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
/// Default spawn for csme: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_csme(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Csme::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:ibm/pcspk",
        summary: "IBM PC speaker (PIT channel 2)",
        outputs: &[],
        spawn: spawn_pcspk,
    },
    ModelEntry {
        id: "ip:intel/pmc",
        summary: "Intel Power Management Controller (PMC)",
        outputs: &[],
        spawn: spawn_pmc,
    },
    ModelEntry {
        id: "ip:intel/pmt",
        summary: "Intel Platform Monitoring Technology (PMT)",
        outputs: &[],
        spawn: spawn_pmt,
    },
    ModelEntry {
        id: "ip:intel/p2sb",
        summary: "Intel Primary-to-Sideband Bridge (P2SB)",
        outputs: &[],
        spawn: spawn_p2sb,
    },
    ModelEntry {
        id: "ip:intel/ish",
        summary: "Intel Integrated Sensor Hub (ISH)",
        outputs: &[],
        spawn: spawn_ish,
    },
    ModelEntry {
        id: "ip:intel/csme",
        summary: "Intel Converged Security and Management Engine (CSME)",
        outputs: &[],
        spawn: spawn_csme,
    },
    ModelEntry {
        id: "ip:ibm/pc-testdev",
        summary: "IBM PC test device",
        outputs: &[],
        spawn: spawn_pc_testdev,
    },
    ModelEntry {
        id: "ip:aspeed/ast-lpc",
        summary: "ASPEED AST2500 LPC controller",
        outputs: &[],
        spawn: spawn_ast_lpc,
    },
    ModelEntry {
        id: "ip:nuvoton/npcm-lpc",
        summary: "Nuvoton NPCM7xx LPC controller",
        outputs: &[],
        spawn: spawn_npcm_lpc,
    },
];
