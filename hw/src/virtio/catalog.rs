//! virtio devices: catalog entries (ADR 0025).

use mosaic_assemble::{InstantiateCtx, MaterializeError};
use mosaic_runtime::Machine;

use crate::catalog::ModelEntry;
use crate::virtio::virtio_blk::VirtioBlk;
use crate::virtio::virtio_net::VirtioNet;

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
];
