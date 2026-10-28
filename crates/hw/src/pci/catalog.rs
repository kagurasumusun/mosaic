//! Catalog entries for `pci` devices (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::pci::ls_pcie::LsPcie;
use crate::pci::mtk_pcie::MtkPcie;
use crate::pci::pci_host_cam::PciHostCam;
use crate::pci::pci_host_ecam::PciHostEcam;
use crate::pci::pci_host_gp::PciHostGp;
use crate::pci::pcie_designware::PcieDesignware;
use crate::pci::pcie_host::PcieHost;
use crate::pci::rk_pcie::RkPcie;

/// Default spawn for pci-host-gp: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pci_host_gp(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PciHostGp::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pci-host-cam: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pci_host_cam(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PciHostCam::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pci-host-ecam: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pci_host_ecam(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PciHostEcam::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pcie-designware: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pcie_designware(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PcieDesignware::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for pcie-host: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_pcie_host(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = PcieHost::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for ls-pcie: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_ls_pcie(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = LsPcie::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for mtk-pcie: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_mtk_pcie(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = MtkPcie::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for rk-pcie: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_rk_pcie(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = RkPcie::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:intel/pci-host-gp",
        summary: "Intel GP PCI host bridge",
        outputs: &[],
        spawn: spawn_pci_host_gp,
    },
    ModelEntry {
        id: "ip:arm/pci-host-cam",
        summary: "ARM Cambridgeshire PCI host bridge",
        outputs: &[],
        spawn: spawn_pci_host_cam,
    },
    ModelEntry {
        id: "ip:arm/pci-host-ecam",
        summary: "ARM ECAM PCI host bridge",
        outputs: &[],
        spawn: spawn_pci_host_ecam,
    },
    ModelEntry {
        id: "ip:synopsys/pcie-designware",
        summary: "Synopsys DesignWare PCIe controller",
        outputs: &[],
        spawn: spawn_pcie_designware,
    },
    ModelEntry {
        id: "ip:cadence/pcie-host",
        summary: "Cadence PCIe host controller",
        outputs: &[],
        spawn: spawn_pcie_host,
    },
    ModelEntry {
        id: "ip:nxp/ls-pcie",
        summary: "NXP Layerscape PCIe controller",
        outputs: &[],
        spawn: spawn_ls_pcie,
    },
    ModelEntry {
        id: "ip:mediatek/mtk-pcie",
        summary: "MediaTek PCIe controller",
        outputs: &[],
        spawn: spawn_mtk_pcie,
    },
    ModelEntry {
        id: "ip:rockchip/rk-pcie",
        summary: "Rockchip PCIe controller",
        outputs: &[],
        spawn: spawn_rk_pcie,
    },
];
