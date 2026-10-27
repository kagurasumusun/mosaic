//! virtio devices: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::virtio::virtio_9p::Virtio9P;
use crate::virtio::virtio_balloon::VirtioBalloon;
use crate::virtio::virtio_blk::VirtioBlk;
use crate::virtio::virtio_console::VirtioConsole;
use crate::virtio::virtio_gpu::VirtioGpu;
use crate::virtio::virtio_input::VirtioInput;
use crate::virtio::virtio_mmio::VirtioMmio;
use crate::virtio::virtio_net::VirtioNet;
use crate::virtio::virtio_pci_legacy::VirtioPciLegacy;
use crate::virtio::virtio_pci_modern::VirtioPciModern;
use crate::virtio::virtio_rng::VirtioRng;
use crate::virtio::virtio_scsi::VirtioScsi;

fn spawn_virtio_blk(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(VirtioBlk::new()))?;
    Ok(())
}

fn spawn_virtio_net(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    machine.register_device(ctx.node, Box::new(VirtioNet::new()))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:virtio/virtio-blk",
        summary: "virtio-blk device (virtio 1.0 register model, feature bits)",
        outputs: &[],
        spawn: spawn_virtio_blk,
    },
    ModelEntry {
        id: "ip:virtio/virtio-net",
        summary: "virtio-net device (virtio 1.0 register model, MAC feature)",
        outputs: &[],
        spawn: spawn_virtio_net,
    },
    ModelEntry {
        id: "ip:virtio/virtio-mmio",
        summary: "VirtIO MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_mmio,
    },
    ModelEntry {
        id: "ip:virtio/virtio-pci-modern",
        summary: "VirtIO PCI modern transport",
        outputs: &[],
        spawn: spawn_virtio_pci_modern,
    },
    ModelEntry {
        id: "ip:virtio/virtio-pci-legacy",
        summary: "VirtIO PCI legacy transport",
        outputs: &[],
        spawn: spawn_virtio_pci_legacy,
    },
    ModelEntry {
        id: "ip:virtio/virtio-console",
        summary: "VirtIO console device",
        outputs: &[],
        spawn: spawn_virtio_console,
    },
    ModelEntry {
        id: "ip:virtio/virtio-rng",
        summary: "VirtIO RNG device",
        outputs: &[],
        spawn: spawn_virtio_rng,
    },
    ModelEntry {
        id: "ip:virtio/virtio-balloon",
        summary: "VirtIO memory balloon",
        outputs: &[],
        spawn: spawn_virtio_balloon,
    },
    ModelEntry {
        id: "ip:virtio/virtio-input",
        summary: "VirtIO input device",
        outputs: &[],
        spawn: spawn_virtio_input,
    },
    ModelEntry {
        id: "ip:virtio/virtio-gpu",
        summary: "VirtIO GPU device",
        outputs: &[],
        spawn: spawn_virtio_gpu,
    },
    ModelEntry {
        id: "ip:virtio/virtio-scsi",
        summary: "VirtIO SCSI device",
        outputs: &[],
        spawn: spawn_virtio_scsi,
    },
    ModelEntry {
        id: "ip:virtio/virtio-9p",
        summary: "VirtIO 9p filesystem device",
        outputs: &[],
        spawn: spawn_virtio_9p,
    },
];

/// Default spawn for virtio-mmio: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_mmio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioMmio::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-pci-modern: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_pci_modern(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioPciModern::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-pci-legacy: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_pci_legacy(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioPciLegacy::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-console: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_console(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioConsole::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-rng: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_rng(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioRng::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-balloon: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_balloon(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioBalloon::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-input: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_input(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioInput::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-gpu: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_gpu(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioGpu::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-scsi: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_scsi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioScsi::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

/// Default spawn for virtio-9p: reads the irq output from the catalog
/// output table and registers the device with the machine.
fn spawn_virtio_9p(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = Virtio9P::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}
