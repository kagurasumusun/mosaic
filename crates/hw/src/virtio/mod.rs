//! virtio devices (QEMU `hw/virtio/`).

pub mod catalog;
pub mod virtio_blk;
pub mod virtio_net;

pub mod virtio_9p;
pub mod virtio_balloon;
pub mod virtio_console;
pub mod virtio_gpu;
pub mod virtio_input;
pub mod virtio_mmio;
pub mod virtio_pci_legacy;
pub mod virtio_pci_modern;
pub mod virtio_rng;
pub mod virtio_scsi;
