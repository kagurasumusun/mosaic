//! The transport ⇄ device port — virtio's own split, in one trait
//! (ADR 0033 §2).
//!
//! The transport (virtio-mmio here, virtio-pci later) owns the register
//! file, the feature negotiation protocol and the queue *addresses*; the
//! device owns everything semantic: which features it offers, its config
//! space, and what a queue kick does. Neither knows the other's layout:
//!
//! * the transport calls [`VirtioDevice`] for every semantic question
//!   (feature bits, config reads/writes, processing one queue) and
//!   forwards exactly two device→transport requests — raise the
//!   interrupt line, and announce a config change — through the
//!   [`VirtioIrq`] port it hands each call;
//! * the device sees a queue as a [`SplitVirtqueue`] view over the
//!   shared [`GuestMemory`], never as raw registers.
//!
//! Processing model: the transport defers every kick to the machine's
//! event queue (a zero-delay timer), so ring processing runs **after**
//! the CPU's stores have left the engine's private pages — the ordering
//! the shared-RAM contract requires. A device's `process` may therefore
//! read the avail ring the driver just wrote, and its completions are
//! visible to the next quantum. Devices must not busy-wait inside
//! `process`; a device that cannot finish now (a host block backend
//! slower than the guest) completes what it can and leaves the rest
//! queued, exactly like real hardware defers to its own engine.

use mosaic_contract::model::Ctx;
use mosaic_exec::{NodeId, SignalId};

use super::vqueue::{GuestMemory, QueueConfig, QueueCursor, QueueError, SplitVirtqueue};

/// Virtio device IDs (virtio 1.1 appendix).
pub mod device_id {
    pub const NET: u32 = 1;
    pub const BLOCK: u32 = 2;
    pub const CONSOLE: u32 = 3;
    pub const ENTROPY: u32 = 4;
    pub const BALLOON: u32 = 5;
    pub const SCSI: u32 = 8;
    pub const _9P: u32 = 9;
    pub const RNG: u32 = 4;
    pub const GPU: u32 = 16;
    pub const INPUT: u32 = 18;
}

/// Feature bits the framework itself negotiates.
pub mod features {
    /// VIRTIO_F_VERSION_1 (bit 32): the modern ring layout.
    pub const VERSION_1: u64 = 1 << 32;
    /// VIRTIO_F_ACCESS_PLATFORM (bit 33): DMA goes through the machine's
    /// guest-memory port (always true here; the bit keeps Linux's DMA
    /// map/unmap calls quiet).
    pub const ACCESS_PLATFORM: u64 = 1 << 33;
    /// VIRTIO_F_RING_EVENT_IDX (bit 29): used_event/avail_event
    /// suppression — supported by the ring core.
    pub const RING_EVENT_IDX: u64 = 1 << 29;
}

/// The transport-side interrupt port handed to devices.
pub struct VirtioIrq<'a> {
    signal: SignalId,
    node: NodeId,
    interrupt_status: &'a mut u32,
}

impl<'a> VirtioIrq<'a> {
    /// Build the port the transport hands a device: the IRQ output, the
    /// owning node (for node-originated signals), and the transport's
    /// interrupt-status word.
    pub fn new(signal: SignalId, node: NodeId, interrupt_status: &'a mut u32) -> Self {
        Self {
            signal,
            node,
            interrupt_status,
        }
    }

    /// Set the queue-interrupt bit (0x1) and raise the line.
    pub fn queue_interrupt(&mut self) {
        *self.interrupt_status |= 1;
        let _ = self.node;
        // The line raise goes through the transport's signal table at
        // return; the bit is what survives an ACK.
    }

    /// Set the config-change bit (0x2) and raise the line.
    pub fn config_interrupt(&mut self) {
        *self.interrupt_status |= 2;
    }

    /// The output signal the transport wired for this device.
    pub fn signal(&self) -> SignalId {
        self.signal
    }
}

/// A complete queue image: config plus cursor, as the transport holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueSlot {
    pub config: QueueConfig,
    pub cursor: QueueCursor,
}

impl QueueSlot {
    pub fn new() -> Self {
        Self {
            config: QueueConfig::default(),
            cursor: QueueCursor::default(),
        }
    }

    pub fn view(&mut self) -> SplitVirtqueue<'_> {
        SplitVirtqueue::new(&self.config, &mut self.cursor)
    }
}

impl Default for QueueSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// A device error surfaced through `process`.
pub type ProcessError = QueueError;

/// The semantic half of one virtio function.
pub trait VirtioDevice: Send {
    /// virtio device ID (1 = net, 2 = block, ...).
    fn device_id(&self) -> u32;

    /// Vendor ID the transport reports (default: the QEMU-style 0x51467701
    /// so existing drivers match a known ecosystem).
    fn vendor_id(&self) -> u32 {
        0x5146_7701
    }

    /// The device feature bits offered (always including VERSION_1 unless
    /// the device says otherwise through this exact answer).
    fn device_features(&self) -> u64 {
        features::VERSION_1
    }

    /// The driver selected features (subset of the offered bits).
    fn set_driver_features(&mut self, _features: u64) {}

    /// The largest queue size this device supports.
    fn max_queue_size(&self) -> u32 {
        128
    }

    /// The number of virtqueues (in queue-index order).
    fn queue_count(&self) -> usize;

    /// Read the config space at `offset` (device-specific layout).
    fn config_read(&self, offset: u64, size: u8) -> u64;

    /// Write the config space at `offset`.
    fn config_write(&mut self, offset: u64, size: u8, value: u64);

    /// Process one queue kick. `queue` is the queue index; the slot's
    /// config and cursor are the transport's live image. `mem` is the
    /// shared guest memory. `irq` sets interrupt bits; `ctx` is the
    /// machine context the event drain handed the transport (devices use
    /// it for host streams and signals, never for guest memory).
    ///
    /// A default implementation that does nothing keeps skeleton devices
    /// compiling; every real device overrides it.
    fn process(
        &mut self,
        _ctx: &mut dyn Ctx,
        _queue: usize,
        _slot: &mut QueueSlot,
        _mem: &mut dyn GuestMemory,
        _irq: &mut VirtioIrq<'_>,
    ) -> Result<(), ProcessError> {
        Ok(())
    }

    /// The guest finished with the device (status reset or FAILED):
    /// drop runtime state that a re-init must not inherit.
    fn reset(&mut self) {}
}
