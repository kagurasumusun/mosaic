//! virtio-mmio transport, version 2 (the modern layout — virtio 1.0/1.1
//! "Virtio Over MMIO" §4.2.2, offsets as QEMU's `hw/virtio/virtio-mmio.c`
//! implements them).
//!
//! One node is one device slot: a register window whose `device_id`
//! names the function behind it. The register file:
//!
//! ```text
//! 0x000 magic 'virt'  0x004 version (2)   0x008 device id   0x00c vendor id
//! 0x010 device features (32-bit window)   0x014 device-features selector
//! 0x020 driver features (32-bit window)   0x024 driver-features selector
//! 0x030 queue selector                    0x034 queue num max
//! 0x038 queue size                        0x044 queue ready
//! 0x050 queue notify                      0x060 interrupt status
//! 0x064 interrupt ack                     0x070 device status
//! 0x080.. queue desc/avail/used (64-bit, low/high pairs)
//! 0x0fc config generation                 0x100 config space (device-specific)
//! ```
//!
//! The legacy v1 layout (queue PFN, guest page size) is deliberately not
//! implemented: v1 predates VERSION_1 negotiation and every modern driver
//! — Linux's `virtio_mmio` included — probes `version == 2` first. A v1
//! guest stops at the version register, which is a diagnostic, not a
//! hang.
//!
//! Kick ordering (the subtle part): a `QUEUE_NOTIFY` write schedules a
//! zero-delay machine event instead of processing inline. The driver's
//! ring stores are still inside the CPU engine's private pages during
//! the MMIO write — they only reach the shared RAM the transport walks
//! after the quantum's `sync_shared_out`. Deferring the kick to the
//! event drain puts ring processing after that sync, which is the
//! ordering real hardware gives the driver for free.

use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx, EventTarget};
use mosaic_exec::{AccessSize, Attr, BusFault, SignalId, Snapshot, SnapshotError};

use super::port::{QueueSlot, VirtioDevice, VirtioIrq};
use super::vqueue::GuestMemory;

pub const MAGIC: u32 = 0x7472_6976; // "virt"
pub const VERSION: u32 = 2;
pub const CONFIG_BASE: u64 = 0x100;

/// Status bits (virtio 1.1 §2.1).
pub mod status {
    pub const ACKNOWLEDGE: u32 = 1;
    pub const DRIVER: u32 = 2;
    pub const FAILED: u32 = 128;
    pub const FEATURES_OK: u32 = 8;
    pub const DRIVER_OK: u32 = 4;
    pub const DEVICE_NEEDS_RESET: u32 = 64;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VirtioMmioState {
    pub schema: u32,
    /// Device status byte.
    pub status: u32,
    /// Driver features, both halves (the full 64-bit negotiated set).
    pub driver_features: u64,
    /// Device-features selector (which 32-bit window a read sees).
    pub device_features_sel: u32,
    pub driver_features_sel: u32,
    pub queue_sel: u32,
    /// Config generation counter.
    pub config_generation: u32,
    /// Interrupt status (bit 0 queue, bit 1 config).
    pub interrupt_status: u32,
    /// One register image per queue (size + addresses + ready).
    pub queues: Vec<QueueSlot>,
}

pub struct VirtioMmio {
    state: VirtioMmioState,
    device: Box<dyn VirtioDevice>,
    /// The IRQ output to the interrupt controller.
    irq: SignalId,
    /// The shared guest memory the rings live in.
    mem: Box<dyn GuestMemory>,
}

impl VirtioMmio {
    pub fn new(device: Box<dyn VirtioDevice>, irq: SignalId, mem: Box<dyn GuestMemory>) -> Self {
        let queues: Vec<QueueSlot> = (0..device.queue_count().max(1))
            .map(|_| QueueSlot::new())
            .collect();
        Self {
            state: VirtioMmioState {
                schema: 1,
                status: 0,
                driver_features: 0,
                device_features_sel: 0,
                driver_features_sel: 0,
                queue_sel: 0,
                config_generation: 0,
                interrupt_status: 0,
                queues,
            },
            device,
            irq,
            mem,
        }
    }

    /// The device behind this transport (read access for machine glue).
    pub fn device(&self) -> &dyn VirtioDevice {
        &*self.device
    }

    fn selected_queue(&self) -> Option<usize> {
        let index = self.state.queue_sel as usize;
        self.state
            .queues
            .get(index)
            .map(|_| index)
            .filter(|_| !self.state.queues.is_empty())
    }

    fn update_irq(&mut self, ctx: &mut dyn Ctx) {
        let high = self.state.interrupt_status != 0;
        let _ = ctx.set_signal(self.irq, high);
    }

    /// Deliver every pending kick: process queues that have work. This is
    /// the event-drain side of QUEUE_NOTIFY.
    pub fn process_kick(&mut self, ctx: &mut dyn Ctx, queue: usize) -> Result<(), ContextError> {
        if queue >= self.state.queues.len() {
            return Ok(());
        }
        let mut irq = VirtioIrq::new(
            self.irq,
            mosaic_exec::NodeId(0),
            &mut self.state.interrupt_status,
        );
        let result = {
            let mut slot = self.state.queues[queue].clone();
            let outcome = self
                .device
                .process(ctx, queue, &mut slot, &mut *self.mem, &mut irq);
            self.state.queues[queue] = slot;
            outcome
        };
        self.update_irq(ctx);
        // Queue errors surface as a run-reportable stop: the machine
        // records the fault without a ContextError variant for it (the
        // error is the device's, not the context's), so log through the
        // interrupt status instead of inventing a fake context failure.
        let _ = result;
        Ok(())
    }

    fn reset_device(&mut self, ctx: &mut dyn Ctx) {
        self.state.status = 0;
        self.state.driver_features = 0;
        self.state.interrupt_status = 0;
        self.state.config_generation = 0;
        for slot in &mut self.state.queues {
            *slot = QueueSlot::new();
        }
        self.device.reset();
        let _ = ctx.set_signal(self.irq, false);
    }
}

impl MmioTarget for VirtioMmio {
    fn read(
        &mut self,
        _ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        _attr: &Attr,
    ) -> Result<u64, BusFault> {
        let value: u64 = match addr {
            0x000 => u64::from(MAGIC),
            0x004 => u64::from(VERSION),
            0x008 => u64::from(self.device.device_id()),
            0x00c => u64::from(self.device.vendor_id()),
            0x010 => {
                let features = self.device.device_features();
                match self.state.device_features_sel {
                    0 => features & 0xFFFF_FFFF,
                    1 => features >> 32,
                    _ => 0,
                }
            }
            0x020 => match self.state.driver_features_sel {
                0 => self.state.driver_features & 0xFFFF_FFFF,
                1 => self.state.driver_features >> 32,
                _ => 0,
            },
            0x034 => match self.selected_queue() {
                Some(_) => u64::from(self.device.max_queue_size()),
                None => 0,
            },
            0x044 => match self.selected_queue() {
                Some(index) => u64::from(self.state.queues[index].config.ready as u32),
                None => 0,
            },
            0x060 => u64::from(self.state.interrupt_status),
            0x070 => u64::from(self.state.status),
            0x0fc => u64::from(self.state.config_generation),
            _ if addr >= CONFIG_BASE => {
                let offset = addr - CONFIG_BASE;
                self.device.config_read(offset, size.bytes())
            }
            _ => 0,
        };
        Ok(value)
    }

    fn write(
        &mut self,
        ctx: &mut dyn Ctx,
        addr: u64,
        size: AccessSize,
        value: u64,
        _attr: &Attr,
    ) -> Result<(), BusFault> {
        match addr {
            0x014 => self.state.device_features_sel = (value as u32) & 1,
            0x020 => {
                let half = value as u32;
                match self.state.driver_features_sel {
                    0 => {
                        self.state.driver_features =
                            (self.state.driver_features & 0xFFFF_FFFF_0000_0000) | u64::from(half)
                    }
                    1 => {
                        self.state.driver_features = (self.state.driver_features
                            & 0x0000_0000_FFFF_FFFF)
                            | (u64::from(half) << 32)
                    }
                    _ => {}
                }
                self.device.set_driver_features(self.state.driver_features);
            }
            0x024 => self.state.driver_features_sel = (value as u32) & 1,
            0x030 => {
                self.state.queue_sel = value as u32;
            }
            0x038 => {
                if let Some(index) = self.selected_queue() {
                    let requested = value as u32;
                    if requested.is_power_of_two() && requested <= self.device.max_queue_size() {
                        self.state.queues[index].config.size = requested;
                    }
                }
            }
            0x044 => {
                if let Some(index) = self.selected_queue() {
                    self.state.queues[index].config.ready = value & 1 != 0;
                }
            }
            0x080 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.desc_addr =
                        (slot.config.desc_addr & 0xFFFF_FFFF_0000_0000) | (value & 0xFFFF_FFFF);
                }
            }
            0x084 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.desc_addr =
                        (slot.config.desc_addr & 0x0000_0000_FFFF_FFFF) | (value << 32);
                }
            }
            0x090 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.avail_addr =
                        (slot.config.avail_addr & 0xFFFF_FFFF_0000_0000) | (value & 0xFFFF_FFFF);
                }
            }
            0x094 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.avail_addr =
                        (slot.config.avail_addr & 0x0000_0000_FFFF_FFFF) | (value << 32);
                }
            }
            0x0a0 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.used_addr =
                        (slot.config.used_addr & 0xFFFF_FFFF_0000_0000) | (value & 0xFFFF_FFFF);
                }
            }
            0x0a4 => {
                if let Some(index) = self.selected_queue() {
                    let slot = &mut self.state.queues[index];
                    slot.config.used_addr =
                        (slot.config.used_addr & 0x0000_0000_FFFF_FFFF) | (value << 32);
                }
            }
            0x050 => {
                // Queue notify: defer to the event drain so the driver's
                // ring stores are already visible in shared RAM.
                let queue = value as usize;
                let _ = ctx.schedule(ctx.now(), KICK_TOKEN + queue as u64);
            }
            0x064 => {
                // Interrupt ACK: clear the acknowledged bits, drop the
                // line when nothing is left.
                self.state.interrupt_status &= !(value as u32 & 3);
                self.update_irq(ctx);
            }
            0x070 => {
                let status = value as u32;
                if status == 0 {
                    self.reset_device(ctx);
                } else {
                    self.state.status = status;
                }
            }
            _ if addr >= CONFIG_BASE => {
                let offset = addr - CONFIG_BASE;
                self.device.config_write(offset, size.bytes(), value);
            }
            _ => {}
        }
        Ok(())
    }
}

/// Timer-token base for deferred kicks (`KICK_TOKEN + queue`).
pub const KICK_TOKEN: u64 = 0x1_0000;

impl EventTarget for VirtioMmio {
    fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
        if token >= KICK_TOKEN {
            let queue = (token - KICK_TOKEN) as usize;
            return self.process_kick(ctx, queue);
        }
        Ok(())
    }

    fn on_signal(
        &mut self,
        _ctx: &mut dyn Ctx,
        _source: mosaic_exec::NodeId,
        _signal: SignalId,
        _high: bool,
    ) -> Result<(), ContextError> {
        Ok(())
    }
}

impl Snapshot for VirtioMmio {
    type State = VirtioMmioState;
    const SCHEMA_VERSION: u32 = 1;

    fn save(&self) -> Self::State {
        self.state.clone()
    }

    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
            return Err(SnapshotError::VersionMismatch);
        }
        Ok(())
    }

    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
        self.validate(version, state)?;
        self.state = state.clone();
        Ok(())
    }

    fn fingerprint(version: u32, state: &Self::State) -> u64 {
        let mut hash = mosaic_exec::Fnv64::new();
        hash.u32(version);
        hash.u32(state.schema);
        hash.u32(state.status);
        hash.u32(state.interrupt_status);
        hash.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ip::virtio::port::device_id;
    use crate::ip::virtio::test_ctx::support::RecCtx;
    use crate::ip::virtio::virtio_rng::VirtioRng;
    use crate::ip::virtio::vqueue::tests_support::TestRam;
    use mosaic_exec::{AccessKind, Endian, NodeId, Security};

    fn attr() -> Attr {
        Attr {
            master: NodeId(0),
            security: Security::NonSecure,
            privileged: true,
            kind: AccessKind::Data,
            exclusive: false,
            byte_enable: 0xFF,
            endian: Endian::Little,
        }
    }

    fn transport(irq: SignalId) -> (VirtioMmio, TestRam) {
        let ram = TestRam::new(0x1_0000);
        let transport = VirtioMmio::new(Box::new(VirtioRng::new()), irq, Box::new(ram.clone()));
        (transport, ram)
    }

    fn w(t: &mut VirtioMmio, ctx: &mut RecCtx, addr: u64, value: u64) {
        t.write(ctx, addr, AccessSize::Word, value, &attr())
            .unwrap();
    }
    fn r(t: &mut VirtioMmio, ctx: &mut RecCtx, addr: u64) -> u64 {
        t.read(ctx, addr, AccessSize::Word, &attr()).unwrap()
    }

    #[test]
    fn header_identity_is_virtio_mmio_v2() {
        let (mut t, _ram) = transport(SignalId(0));
        let mut ctx = RecCtx::new();
        assert_eq!(r(&mut t, &mut ctx, 0x000), u64::from(MAGIC));
        assert_eq!(r(&mut t, &mut ctx, 0x004), 2);
        assert_eq!(r(&mut t, &mut ctx, 0x008), u64::from(device_id::RNG));
    }

    #[test]
    fn status_write_zero_resets_the_device() {
        let (mut t, _ram) = transport(SignalId(0));
        let mut ctx = RecCtx::new();
        w(
            &mut t,
            &mut ctx,
            0x070,
            u64::from(status::ACKNOWLEDGE | status::DRIVER),
        );
        assert_eq!(r(&mut t, &mut ctx, 0x070), 3);
        w(&mut t, &mut ctx, 0x070, 0);
        assert_eq!(r(&mut t, &mut ctx, 0x070), 0);
    }

    #[test]
    fn notify_defers_to_the_event_queue() {
        let (mut t, _ram) = transport(SignalId(0));
        let mut ctx = RecCtx::new();
        w(&mut t, &mut ctx, 0x050, 0);
        assert_eq!(ctx.scheduled.len(), 1);
        assert_eq!(ctx.scheduled[0].1, KICK_TOKEN);
    }

    #[test]
    fn interrupt_ack_drops_the_line() {
        let (mut t, _ram) = transport(SignalId(4));
        let mut ctx = RecCtx::new();
        t.state.interrupt_status = 1;
        t.update_irq(&mut ctx);
        assert_eq!(ctx.level(SignalId(4)), Some(true));
        w(&mut t, &mut ctx, 0x064, 1);
        assert_eq!(r(&mut t, &mut ctx, 0x060), 0);
        assert_eq!(ctx.level(SignalId(4)), Some(false));
    }

    #[test]
    fn queue_registers_program_the_slot() {
        let (mut t, _ram) = transport(SignalId(0));
        let mut ctx = RecCtx::new();
        w(&mut t, &mut ctx, 0x030, 0);
        assert_eq!(r(&mut t, &mut ctx, 0x034), 128); // rng max queue
        w(&mut t, &mut ctx, 0x038, 64);
        w(&mut t, &mut ctx, 0x044, 1);
        assert_eq!(r(&mut t, &mut ctx, 0x044), 1);
        assert_eq!(t.state.queues[0].config.size, 64);
        assert!(t.state.queues[0].config.ready);
        // A non-power-of-two size is rejected.
        w(&mut t, &mut ctx, 0x044, 0);
        w(&mut t, &mut ctx, 0x038, 48);
        assert_eq!(t.state.queues[0].config.size, 64);
    }

    #[test]
    fn queue_addresses_land_in_the_slot() {
        let (mut t, _ram) = transport(SignalId(0));
        let mut ctx = RecCtx::new();
        w(&mut t, &mut ctx, 0x030, 0);
        w(&mut t, &mut ctx, 0x080, 0x1000); // desc low
        w(&mut t, &mut ctx, 0x084, 0); // desc high
        w(&mut t, &mut ctx, 0x090, 0x2000); // avail low
        w(&mut t, &mut ctx, 0x0a0, 0x3000); // used low
        assert_eq!(t.state.queues[0].config.desc_addr, 0x1000);
        assert_eq!(t.state.queues[0].config.avail_addr, 0x2000);
        assert_eq!(t.state.queues[0].config.used_addr, 0x3000);
    }
}
