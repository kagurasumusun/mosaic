//! Virtio catalog entries (QEMU `hw/virtio/`'s `-device` roster).
//!
//! Each `ip:virtio/<name>` entry is a **complete device on the MMIO
//! transport**: the spawn wraps the device model in a [`VirtioMmio`]
//! transport, wires the IRQ output, attaches the host block image /
//! console stream / net link / guest-memory port the harness bound to
//! this node, and registers one snapshot-capable machine device. This is
//! QEMU's `virtio-blk-device` shape — the composition point is the
//! catalog, never the board (ADR 0025: a model's default instantiation
//! is the model's own knowledge).
//!
//! `ip:virtio/virtio-mmio` itself is the parameterised transport: its
//! `device` parameter names the function (`blk`, `net`, `console`,
//! `rng`, `input`, `gpu`, `balloon`, `scsi`, `9p`), for boards that
//! place transports explicitly.

use crate::catalog::ModelEntry;
use crate::ip::virtio::port::VirtioDevice;
use crate::ip::virtio::virtio_9p::Virtio9p;
use crate::ip::virtio::virtio_balloon::VirtioBalloon;
use crate::ip::virtio::virtio_blk::VirtioBlk;
use crate::ip::virtio::virtio_console::VirtioConsole;
use crate::ip::virtio::virtio_gpu::VirtioGpu;
use crate::ip::virtio::virtio_input::VirtioInput;
use crate::ip::virtio::virtio_mmio::VirtioMmio;
use crate::ip::virtio::virtio_net::VirtioNet;
use crate::ip::virtio::virtio_pci_legacy::VirtioPciLegacy;
use crate::ip::virtio::virtio_pci_modern::VirtioPciModern;
use crate::ip::virtio::virtio_rng::VirtioRng;
use crate::ip::virtio::virtio_scsi::VirtioScsi;
use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

/// Wrap one device in the MMIO transport and register it. The shared
/// guest-memory port comes from the harness attachment; without one the
/// device is registered with an unusable (always-unmapped) memory port,
/// which a probe surfaces as a fault rather than a hang.
fn spawn_transport(
    device: Box<dyn VirtioDevice>,
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let memory: Box<dyn mosaic_contract::guest_mem::GuestMemory> = match ctx.host_guest_memory() {
        Some(shared) => Box::new(SharedPort(shared)),
        None => Box::new(NoMemory),
    };
    let transport = VirtioMmio::new(device, irq, memory);
    machine.register_snapshot_device(ctx.node, transport)?;
    Ok(())
}

/// A host-shared guest-memory port behind a mutex (the harness attachment
/// shape).
struct SharedPort(std::sync::Arc<std::sync::Mutex<dyn mosaic_contract::guest_mem::GuestMemory>>);

impl mosaic_contract::guest_mem::GuestMemory for SharedPort {
    fn read_bytes(
        &self,
        address: u64,
        out: &mut [u8],
    ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
        self.0
            .lock()
            .map_err(|_| mosaic_contract::guest_mem::GuestMemError::Unmapped { address })?
            .read_bytes(address, out)
    }

    fn write_bytes(
        &mut self,
        address: u64,
        data: &[u8],
    ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
        self.0
            .lock()
            .map_err(|_| mosaic_contract::guest_mem::GuestMemError::Unmapped { address })?
            .write_bytes(address, data)
    }
}

/// The no-attachment port: every access is unmapped, so a misplaced
/// device fails loudly instead of reading zeros.
struct NoMemory;

impl mosaic_contract::guest_mem::GuestMemory for NoMemory {
    fn read_bytes(
        &self,
        address: u64,
        _out: &mut [u8],
    ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
        Err(mosaic_contract::guest_mem::GuestMemError::Unmapped { address })
    }

    fn write_bytes(
        &mut self,
        address: u64,
        _data: &[u8],
    ) -> Result<(), mosaic_contract::guest_mem::GuestMemError> {
        Err(mosaic_contract::guest_mem::GuestMemError::Unmapped { address })
    }
}

fn spawn_virtio_blk(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut device = VirtioBlk::new();
    if let Some(image) = ctx.host_block() {
        // The shared image moves into the device; other holders keep
        // their own attachment (read-only sharing is the harness's call).
        if let Ok(mut guard) = image.lock() {
            device.attach_image(std::mem::replace(&mut *guard, Box::new(EmptyImage)));
        }
    }
    spawn_transport(Box::new(device), ctx, machine)
}

fn spawn_virtio_net(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut device = VirtioNet::new();
    if let Some(link) = ctx.host_net() {
        if let Ok(mut guard) = link.lock() {
            // The link moves into the device (its ports are the wiring).
            device.attach_link(std::mem::take(&mut *guard));
        }
    }
    spawn_transport(Box::new(device), ctx, machine)
}

fn spawn_virtio_console(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let mut device = VirtioConsole::new();
    if let Some(endpoint) = ctx.host_uart() {
        device.attach_stream(Box::new(endpoint.clone()));
    }
    spawn_transport(Box::new(device), ctx, machine)
}

fn spawn_virtio_rng(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let seed = ctx.parameter_int("seed").unwrap_or(0) as u64;
    spawn_transport(Box::new(VirtioRng::with_seed(seed)), ctx, machine)
}

fn spawn_virtio_input(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let name = ctx
        .parameter_str("name")
        .unwrap_or("mosaic-input")
        .to_owned();
    spawn_transport(Box::new(VirtioInput::new(&name)), ctx, machine)
}

fn spawn_virtio_gpu(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    spawn_transport(Box::new(VirtioGpu::new()), ctx, machine)
}

fn spawn_virtio_balloon(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    spawn_transport(Box::new(VirtioBalloon::new()), ctx, machine)
}

fn spawn_virtio_scsi(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let mut device = VirtioScsi::new();
    if let Some(image) = ctx.host_block() {
        if let Ok(mut guard) = image.lock() {
            device.attach_image(std::mem::replace(&mut *guard, Box::new(EmptyImage)));
        }
    }
    spawn_transport(Box::new(device), ctx, machine)
}

fn spawn_virtio_9p(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    spawn_transport(Box::new(Virtio9p::new()), ctx, machine)
}

/// The parameterised transport entry: `device` names the function.
fn spawn_virtio_mmio(ctx: &InstantiateCtx, machine: &mut Machine) -> Result<(), MaterializeError> {
    let device: Box<dyn VirtioDevice> = match ctx.parameter_str("device") {
        Some("blk") => Box::new(VirtioBlk::new()),
        Some("net") => Box::new(VirtioNet::new()),
        Some("console") => Box::new(VirtioConsole::new()),
        Some("rng") => Box::new(VirtioRng::new()),
        Some("input") => Box::new(VirtioInput::new("mosaic-input")),
        Some("gpu") => Box::new(VirtioGpu::new()),
        Some("balloon") => Box::new(VirtioBalloon::new()),
        Some("scsi") => Box::new(VirtioScsi::new()),
        Some("9p") => Box::new(Virtio9p::new()),
        _ => {
            return Err(MaterializeError::MissingFactory {
                catalog_id: "virtio-mmio: parameter device must name a virtio function".to_owned(),
                node: String::new(),
            })
        }
    };
    spawn_transport(device, ctx, machine)
}

/// A placeholder left in the harness's shared slot after the device took
/// the real image (the attachment is consumed once).
struct EmptyImage;

impl mosaic_contract::block::BlockImage for EmptyImage {
    fn read_at(
        &self,
        _offset: u64,
        buf: &mut [u8],
    ) -> Result<(), mosaic_contract::block::BlockError> {
        buf.fill(0);
        Ok(())
    }
    fn write_at(
        &mut self,
        _offset: u64,
        _data: &[u8],
    ) -> Result<(), mosaic_contract::block::BlockError> {
        Err(mosaic_contract::block::BlockError::Io)
    }
    fn size(&self) -> u64 {
        0
    }
}

/// Default spawn for the PCI transports (register models until the PCI
/// fabric wave, ADR 0033 wave 4).
fn spawn_virtio_pci_modern(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioPciModern::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

fn spawn_virtio_pci_legacy(
    ctx: &InstantiateCtx,
    machine: &mut Machine,
) -> Result<(), MaterializeError> {
    let irq = ctx.output("irq")?;
    let device = VirtioPciLegacy::new(irq);
    machine.register_device(ctx.node, Box::new(device))?;
    Ok(())
}

pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        id: "ip:virtio/virtio-blk",
        summary: "VirtIO block device on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_blk,
    },
    ModelEntry {
        id: "ip:virtio/virtio-net",
        summary: "VirtIO network device on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_net,
    },
    ModelEntry {
        id: "ip:virtio/virtio-console",
        summary: "VirtIO console on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_console,
    },
    ModelEntry {
        id: "ip:virtio/virtio-rng",
        summary: "VirtIO entropy source on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_rng,
    },
    ModelEntry {
        id: "ip:virtio/virtio-input",
        summary: "VirtIO evdev input device on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_input,
    },
    ModelEntry {
        id: "ip:virtio/virtio-gpu",
        summary: "VirtIO 2D GPU on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_gpu,
    },
    ModelEntry {
        id: "ip:virtio/virtio-balloon",
        summary: "VirtIO memory balloon on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_balloon,
    },
    ModelEntry {
        id: "ip:virtio/virtio-scsi",
        summary: "VirtIO SCSI host on the MMIO transport",
        outputs: &[],
        spawn: spawn_virtio_scsi,
    },
    ModelEntry {
        id: "ip:virtio/virtio-9p",
        summary:
            "VirtIO 9P share on the MMIO transport (answers RLerror until the host layer exists)",
        outputs: &[],
        spawn: spawn_virtio_9p,
    },
    ModelEntry {
        id: "ip:virtio/virtio-mmio",
        summary: "VirtIO MMIO transport, device chosen by the `device` parameter",
        outputs: &[],
        spawn: spawn_virtio_mmio,
    },
    ModelEntry {
        id: "ip:virtio/virtio-pci-modern",
        summary: "VirtIO PCI modern transport (register model)",
        outputs: &[],
        spawn: spawn_virtio_pci_modern,
    },
    ModelEntry {
        id: "ip:virtio/virtio-pci-legacy",
        summary: "VirtIO PCI legacy transport (register model)",
        outputs: &[],
        spawn: spawn_virtio_pci_legacy,
    },
];
