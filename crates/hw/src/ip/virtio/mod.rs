//! Virtio device models (QEMU `hw/virtio/`), in three layers (ADR 0033):
//! `vqueue` (the split-ring core), `port` (the transport ⇄ device
//! contract), the transports (`virtio_mmio`, `virtio_pci_*`) and the
//! devices (`virtio_blk`, ...).
pub mod catalog;
pub mod port;
#[cfg(test)]
pub mod test_ctx;
pub mod virtio_9p;
pub mod virtio_balloon;
pub mod virtio_blk;
pub mod virtio_console;
pub mod virtio_gpu;
pub mod virtio_input;
pub mod virtio_mmio;
pub mod virtio_net;
pub mod virtio_pci_legacy;
pub mod virtio_pci_modern;
pub mod virtio_rng;
pub mod virtio_scsi;
pub mod vqueue;
