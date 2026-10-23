//! Single-threaded machine: address resolution, MMIO dispatch and event delivery.
//!
//! The machine owns the symbolic [`FlatView`], the device arena, RAM/ROM
//! backing blocks, the deterministic event queue and the signal wiring table.
//! Models never reference each other: bus accesses resolve to a node id and
//! device-local offset, timer events reach nodes through the queue, and signal
//! changes are delivered only after the originating callback has returned.
//!
//! This is not yet a whole-machine emulator: CPU engines, DMI host pointers,
//! clock trees and persistence of device state are integrated later. It is the
//! runtime groundwork that those integrations dispatch through.

use std::collections::{BTreeMap, BTreeSet};

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, EventTarget};
use mosaic_contract_signal::SignalSink;
use mosaic_core_types::{
    AccessKind, AccessSize, Addr, Attr, BusFault, Endian, EventId, MemoryKind, NodeId, Security,
    SignalId, Snapshot, SnapshotError, VTime,
};

use crate::address::{Access, FlatView, Resolved};
use crate::arena::DeviceArena;
use crate::backing::BackingStore;
use crate::context::ModelContext;
use crate::events::{Event, EventKind, EventQueue, QueueState};

/// Runtime composition of the contract facets a bus-driven model needs.
///
/// One boxed value serves both MMIO and timer callbacks, so register state is
/// never split between trait objects. The blanket impl keeps models writing
/// only the contract traits from `mosaic-contract-*`.
pub trait MachineDevice: MmioTarget + EventTarget {}
impl<T: MmioTarget + EventTarget> MachineDevice for T {}

/// Construction and dispatch failures. Bus failures keep the shared
/// [`BusFault`] vocabulary so device and machine errors use one type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum MachineError {
    DuplicateNode(NodeId),
    UnknownNode(NodeId),
    ZeroSizeBlock(NodeId),
    BackingTooLarge {
        node: NodeId,
        size: u64,
    },
    BackingTooSmall {
        node: NodeId,
        needed: u128,
        available: u64,
    },
    MissingSink(NodeId),
    NoTimerHandler(NodeId),
    UnmappedTarget(NodeId),
    EventBudgetExceeded,
    Context(ContextError),
}

impl From<ContextError> for MachineError {
    fn from(value: ContextError) -> Self {
        Self::Context(value)
    }
}

pub struct Machine {
    queue: EventQueue,
    view: FlatView,
    arena: DeviceArena,
    sinks: BTreeMap<u32, Box<dyn SignalSink>>,
    backing: BackingStore,
    /// Per-node outgoing signal ids, derived from the wiring declarations.
    outputs: BTreeMap<u32, BTreeSet<u32>>,
    /// `(source node, signal id) -> sink node`.
    wiring: BTreeMap<(u32, u32), u32>,
}

impl Machine {
    pub fn new(view: FlatView, queue_capacity: usize) -> Self {
        Self {
            queue: EventQueue::new(queue_capacity),
            view,
            arena: DeviceArena::new(),
            sinks: BTreeMap::new(),
            backing: BackingStore::new(),
            outputs: BTreeMap::new(),
            wiring: BTreeMap::new(),
        }
    }

    pub fn now(&self) -> VTime {
        self.queue.now()
    }

    pub fn pending_events(&self) -> usize {
        self.queue.len()
    }

    pub fn register_device(
        &mut self,
        node: NodeId,
        device: Box<dyn MachineDevice>,
    ) -> Result<(), MachineError> {
        self.arena.insert(node, device)
    }

    pub fn register_sink(
        &mut self,
        node: NodeId,
        sink: Box<dyn SignalSink>,
    ) -> Result<(), MachineError> {
        if self.sinks.contains_key(&node.0) {
            return Err(MachineError::DuplicateNode(node));
        }
        self.sinks.insert(node.0, sink);
        Ok(())
    }

    pub fn add_backing(&mut self, node: NodeId, size: u64) -> Result<(), MachineError> {
        self.backing.add_block(node, size)
    }

    /// Declare that `source`'s output `signal` drives `sink`. The source must
    /// own a registered device and the sink must exist, so wiring mistakes fail
    /// at construction time rather than as silently dropped guest interrupts.
    pub fn connect(
        &mut self,
        source: NodeId,
        signal: SignalId,
        sink: NodeId,
    ) -> Result<(), MachineError> {
        if !self.arena.is_registered(source) {
            return Err(MachineError::UnknownNode(source));
        }
        if !self.sinks.contains_key(&sink.0) {
            return Err(MachineError::MissingSink(sink));
        }
        self.outputs.entry(source.0).or_default().insert(signal.0);
        self.wiring.insert((source.0, signal.0), sink.0);
        Ok(())
    }

    /// Cross-check the view against the registered nodes: every Ram/Rom mapping
    /// needs a sufficiently large backing block, every Mmio mapping a device.
    pub fn verify(&self) -> Result<(), MachineError> {
        for mapping in self.view.mappings() {
            match mapping.kind {
                MemoryKind::Ram | MemoryKind::Rom => {
                    let available = self
                        .backing
                        .len_of(mapping.target)
                        .ok_or(MachineError::UnknownNode(mapping.target))?;
                    let needed = u128::from(mapping.offset) + u128::from(mapping.size);
                    if needed > u128::from(available) {
                        return Err(MachineError::BackingTooSmall {
                            node: mapping.target,
                            needed,
                            available,
                        });
                    }
                }
                MemoryKind::Mmio => {
                    if !self.arena.is_registered(mapping.target) {
                        return Err(MachineError::UnmappedTarget(mapping.target));
                    }
                }
            }
        }
        Ok(())
    }

    /// Guest or DMA bus read. Attr travels unchanged to the device callback.
    pub fn bus_read(
        &mut self,
        address: Addr,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault> {
        check_alignment(address, size)?;
        let resolved = self.view.resolve(address, size, Access::Read)?;
        match resolved.kind {
            MemoryKind::Ram | MemoryKind::Rom => {
                self.backing
                    .guest_read(resolved.target, resolved.offset, size, attr.endian)
            }
            MemoryKind::Mmio => self.dispatch_mmio_read(resolved, size, attr),
        }
    }

    /// Guest or DMA bus write. The view, not the device, owns permissions.
    pub fn bus_write(
        &mut self,
        address: Addr,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault> {
        check_alignment(address, size)?;
        let resolved = self.view.resolve(address, size, Access::Write)?;
        match resolved.kind {
            MemoryKind::Ram | MemoryKind::Rom => {
                self.backing
                    .guest_write(resolved.target, resolved.offset, size, value, attr.endian)
            }
            MemoryKind::Mmio => self.dispatch_mmio_write(resolved, size, value, attr),
        }
    }

    /// Loader/debug initialization path; Rom blocks are writable here.
    pub fn loader_write(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), BusFault> {
        self.backing.loader_write(node, offset, bytes)
    }

    pub fn loader_read(&self, node: NodeId, offset: u64, bytes: &mut [u8]) -> Result<(), BusFault> {
        self.backing.loader_read(node, offset, bytes)
    }

    fn dispatch_mmio_read(
        &mut self,
        resolved: Resolved,
        size: AccessSize,
        attr: &Attr,
    ) -> Result<u64, BusFault> {
        let node = resolved.target;
        let Some(mut device) = self.arena.take(node) else {
            return Err(self.absent_fault(node));
        };
        let result = {
            let empty = BTreeSet::new();
            let outputs = self.outputs.get(&node.0).unwrap_or(&empty);
            let mut ctx = ModelContext::new(&mut self.queue, node, outputs);
            device.read(&mut ctx, resolved.offset, size, attr)
        };
        self.arena.restore(node, device);
        result
    }

    fn dispatch_mmio_write(
        &mut self,
        resolved: Resolved,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault> {
        let node = resolved.target;
        let Some(mut device) = self.arena.take(node) else {
            return Err(self.absent_fault(node));
        };
        let result = {
            let empty = BTreeSet::new();
            let outputs = self.outputs.get(&node.0).unwrap_or(&empty);
            let mut ctx = ModelContext::new(&mut self.queue, node, outputs);
            device.write(&mut ctx, resolved.offset, size, value, attr)
        };
        self.arena.restore(node, device);
        result
    }

    fn absent_fault(&self, node: NodeId) -> BusFault {
        if self.arena.is_registered(node) {
            // The slot exists but its device is taken by an outer callback.
            BusFault::Reentrant
        } else {
            BusFault::Unmapped
        }
    }

    /// Pop the next due event and deliver it to its node or wired sink.
    pub fn step(&mut self) -> Result<Option<Event>, MachineError> {
        let Some(event) = self.queue.pop_next() else {
            return Ok(None);
        };
        self.dispatch_event(event)?;
        Ok(Some(event))
    }

    /// Deliver events up to `deadline`, bounded by `budget` deliveries. The
    /// budget bounds endless same-time rescheduling; an idle queue simply
    /// advances virtual time to the deadline, matching the queue contract.
    pub fn run_until(&mut self, deadline: VTime, budget: usize) -> Result<usize, MachineError> {
        let mut delivered = 0;
        loop {
            let Some(event) = self.queue.pop_due(deadline)? else {
                return Ok(delivered);
            };
            if delivered >= budget {
                return Err(MachineError::EventBudgetExceeded);
            }
            self.dispatch_event(event)?;
            delivered += 1;
        }
    }

    fn dispatch_event(&mut self, event: Event) -> Result<(), MachineError> {
        let owner = event.owner;
        match event.kind {
            EventKind::Timer { token } => {
                let Some(device) = self.arena.event_target(owner) else {
                    return Err(MachineError::NoTimerHandler(owner));
                };
                let outputs = self.outputs.get(&owner.0).cloned().unwrap_or_default();
                let mut ctx = ModelContext::new(&mut self.queue, owner, &outputs);
                device.on_timer(&mut ctx, token)?;
            }
            EventKind::Signal { signal, high } => {
                if let Some(&sink_node) = self.wiring.get(&(owner.0, signal.0)) {
                    let Some(sink) = self.sinks.get_mut(&sink_node) else {
                        return Err(MachineError::MissingSink(NodeId(sink_node)));
                    };
                    sink.set_level(high);
                }
                // An unconnected output is a valid optional port: no destination.
            }
        }
        Ok(())
    }

    pub fn save_queue(&self) -> QueueState {
        self.queue.save()
    }

    pub fn load_queue(&mut self, state: &QueueState) -> Result<(), SnapshotError> {
        self.queue.load(EventQueue::SCHEMA_VERSION, state)
    }

    /// Reserve a node-local timer without a model callback, used by loaders and
    /// tests. Models schedule through `Ctx` inside their own callbacks.
    pub fn schedule_timer(
        &mut self,
        node: NodeId,
        at: VTime,
        token: u64,
    ) -> Result<EventId, ContextError> {
        self.queue.schedule(node, at, EventKind::Timer { token })
    }
}

/// Default little-endian, non-secure data attribute for a bus master.
pub fn bus_attr(master: NodeId) -> Attr {
    Attr {
        master,
        security: Security::NonSecure,
        privileged: false,
        kind: AccessKind::Data,
        exclusive: false,
        byte_enable: u8::MAX,
        endian: Endian::Little,
    }
}

fn check_alignment(address: Addr, size: AccessSize) -> Result<(), BusFault> {
    if address.0 % u64::from(size.bytes()) == 0 {
        Ok(())
    } else {
        Err(BusFault::Misaligned)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use mosaic_contract_model::Ctx;
    use mosaic_core_types::{Mapping, Permissions};

    // Region constructors live in the runtime address module.
    use crate::address::Region;

    const ROM: NodeId = NodeId(1);
    const RAM: NodeId = NodeId(2);
    const DEVICE: NodeId = NodeId(3);
    const SINK: NodeId = NodeId(4);

    #[derive(Default)]
    struct PacingDevice {
        data_reads: u32,
    }

    impl MmioTarget for PacingDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            attr: &Attr,
        ) -> Result<u64, BusFault> {
            // Debug access must never consume data or mutate device state.
            if attr.kind == AccessKind::Debug {
                return Ok(0);
            }
            if offset >= 16 {
                return Err(BusFault::Device);
            }
            self.data_reads += 1;
            // Encode the bus master in the high word so Attr passthrough is
            // observable without exposing device internals to the test.
            Ok(u64::from(self.data_reads) | (u64::from(attr.master.0) << 32))
        }

        fn write(
            &mut self,
            ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            if offset >= 16 {
                return Err(BusFault::Device);
            }
            ctx.schedule_after(VTime(value.saturating_mul(100)), offset)
                .map_err(|_| BusFault::Device)?;
            Ok(())
        }
    }

    impl EventTarget for PacingDevice {
        fn on_timer(&mut self, ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
            ctx.set_signal(SignalId(1), true)?;
            if token == 7 {
                ctx.set_signal(SignalId(1), false)?;
            }
            Ok(())
        }
    }

    // Test-only shared probe: the boxed sink and the test inspect one vector.
    // Arc/Mutex keeps the sink Send while execution stays single-threaded.
    #[derive(Default, Clone)]
    struct LevelProbe {
        levels: Arc<Mutex<Vec<bool>>>,
    }

    impl SignalSink for LevelProbe {
        fn set_level(&mut self, high: bool) {
            self.levels.lock().expect("probe lock").push(high);
        }
    }

    fn leaf(
        base: u64,
        size: u64,
        node: NodeId,
        kind: MemoryKind,
        permissions: Permissions,
    ) -> Region {
        Region::leaf(
            Mapping {
                base: Addr(base),
                size,
                target: node,
                offset: 0,
                kind,
                permissions,
            },
            0,
        )
    }

    fn build_machine(capacity: usize) -> (Machine, LevelProbe) {
        let view = FlatView::build(&[
            leaf(0, 0x1000, ROM, MemoryKind::Rom, Permissions::RX),
            leaf(0x1000, 0x1000, RAM, MemoryKind::Ram, Permissions::RW),
            leaf(0x2000, 0x1000, DEVICE, MemoryKind::Mmio, Permissions::RW),
        ])
        .unwrap();
        let mut machine = Machine::new(view, capacity);
        machine.add_backing(ROM, 0x1000).unwrap();
        machine.add_backing(RAM, 0x1000).unwrap();
        machine
            .register_device(DEVICE, Box::new(PacingDevice::default()))
            .unwrap();
        let probe = LevelProbe::default();
        machine
            .register_sink(SINK, Box::new(probe.clone()))
            .unwrap();
        machine.connect(DEVICE, SignalId(1), SINK).unwrap();
        machine.verify().unwrap();
        (machine, probe)
    }

    fn levels(probe: &LevelProbe) -> Vec<bool> {
        probe.levels.lock().expect("probe lock").clone()
    }

    #[test]
    fn ram_rom_and_view_permissions_are_enforced() {
        let (mut machine, _) = build_machine(16);
        let attr = bus_attr(RAM);
        machine
            .bus_write(Addr(0x1000), AccessSize::Word, 0x1122_3344, &attr)
            .unwrap();
        assert_eq!(
            machine.bus_read(Addr(0x1000), AccessSize::Word, &attr),
            Ok(0x1122_3344)
        );
        // Loader can initialize ROM; guest writes stay forbidden by the view.
        machine.loader_write(ROM, 0, &[0xaa, 0xbb]).unwrap();
        let mut bytes = [0; 2];
        machine.loader_read(ROM, 0, &mut bytes).unwrap();
        assert_eq!(bytes, [0xaa, 0xbb]);
        assert_eq!(
            machine.bus_write(Addr(0), AccessSize::Byte, 1, &attr),
            Err(BusFault::PermissionDenied)
        );
        assert_eq!(
            machine.bus_read(Addr(0x3000), AccessSize::Byte, &attr),
            Err(BusFault::Unmapped)
        );
        assert_eq!(
            machine.bus_read(Addr(0x1001), AccessSize::HalfWord, &attr),
            Err(BusFault::Misaligned)
        );
        // Big-endian attribute changes only the bus encoding.
        let mut be = bus_attr(RAM);
        be.endian = Endian::Big;
        machine
            .bus_write(Addr(0x1004), AccessSize::HalfWord, 0xabcd, &be)
            .unwrap();
        let mut raw = [0; 2];
        machine.loader_read(RAM, 4, &mut raw).unwrap();
        assert_eq!(raw, [0xab, 0xcd]);
    }

    #[test]
    fn mmio_dispatch_passes_offsets_and_attributes_to_the_device() {
        let (mut machine, _) = build_machine(16);
        let master = NodeId(9);
        let attr = bus_attr(master);
        machine
            .bus_write(Addr(0x2004), AccessSize::Word, 3, &attr)
            .unwrap();
        // Low word is the read counter, high word is the Attr master id.
        assert_eq!(
            machine.bus_read(Addr(0x2008), AccessSize::Word, &attr),
            Ok(1 | (9 << 32))
        );
        let debug = Attr {
            kind: AccessKind::Debug,
            ..attr
        };
        assert_eq!(
            machine.bus_read(Addr(0x2008), AccessSize::Word, &debug),
            Ok(0)
        );
        // The data read counted once; the debug read added no side effect.
        assert_eq!(
            machine.bus_read(Addr(0x2008), AccessSize::Word, &attr),
            Ok(2 | (9 << 32))
        );
        assert_eq!(
            machine.bus_write(Addr(0x2010), AccessSize::Word, 1, &attr),
            Err(BusFault::Device)
        );
    }

    #[test]
    fn taken_device_reports_reentrant_and_is_restored_afterward() {
        let (mut machine, _) = build_machine(16);
        let attr = bus_attr(RAM);
        let device = machine.arena.take(DEVICE).unwrap();
        assert_eq!(
            machine.bus_read(Addr(0x2000), AccessSize::Word, &attr),
            Err(BusFault::Reentrant)
        );
        machine.arena.restore(DEVICE, device);
        assert!(machine
            .bus_read(Addr(0x2000), AccessSize::Word, &attr)
            .is_ok());
    }

    #[test]
    fn timer_events_drive_wired_signals_in_deterministic_order() {
        let (mut machine, probe) = build_machine(32);
        let attr = bus_attr(RAM);
        // value 5 -> timer at 500ps; value 2 -> timer at 200ps.
        machine
            .bus_write(Addr(0x2000), AccessSize::Word, 5, &attr)
            .unwrap();
        machine
            .bus_write(Addr(0x2000), AccessSize::Word, 2, &attr)
            .unwrap();
        // Two timers, and each timer queues one signal event: four deliveries.
        let delivered = machine.run_until(VTime(1000), 32).unwrap();
        assert_eq!(delivered, 4);
        assert_eq!(machine.now(), VTime(1000));
        assert_eq!(levels(&probe), [true, true]);

        // Token 7 raises then lowers at the same time, preserving queue order.
        machine
            .bus_write(Addr(0x2007), AccessSize::Byte, 1, &attr)
            .unwrap();
        machine.run_until(VTime(2000), 32).unwrap();
        assert_eq!(levels(&probe), [true, true, true, false]);
    }

    #[test]
    fn construction_and_verification_reject_broken_wiring() {
        let (mut machine, _) = build_machine(8);
        assert_eq!(
            machine.register_device(DEVICE, Box::new(PacingDevice::default())),
            Err(MachineError::DuplicateNode(DEVICE))
        );
        assert_eq!(
            machine.connect(NodeId(99), SignalId(1), SINK),
            Err(MachineError::UnknownNode(NodeId(99)))
        );
        assert_eq!(
            machine.connect(DEVICE, SignalId(2), NodeId(99)),
            Err(MachineError::MissingSink(NodeId(99)))
        );
        assert_eq!(
            machine.add_backing(NodeId(7), 0),
            Err(MachineError::ZeroSizeBlock(NodeId(7)))
        );

        let view = FlatView::build(&[leaf(
            0,
            0x1000,
            NodeId(8),
            MemoryKind::Mmio,
            Permissions::RW,
        )])
        .unwrap();
        let mut incomplete = Machine::new(view, 8);
        assert_eq!(
            incomplete.verify(),
            Err(MachineError::UnmappedTarget(NodeId(8)))
        );
        incomplete.add_backing(NodeId(8), 0x800).unwrap();
        // Backing exists but is smaller than the declared mapping.
        let view = FlatView::build(&[leaf(0, 0x1000, NodeId(8), MemoryKind::Ram, Permissions::RW)])
            .unwrap();
        let mut undersized = Machine::new(view, 8);
        undersized.add_backing(NodeId(8), 0x800).unwrap();
        assert!(matches!(
            undersized.verify(),
            Err(MachineError::BackingTooSmall { .. })
        ));
    }

    #[test]
    fn event_budget_bounds_endless_same_time_rescheduling() {
        #[derive(Default)]
        struct LoopDevice;
        impl MmioTarget for LoopDevice {
            fn read(
                &mut self,
                _ctx: &mut dyn Ctx,
                _offset: u64,
                _size: AccessSize,
                _attr: &Attr,
            ) -> Result<u64, BusFault> {
                Ok(0)
            }
            fn write(
                &mut self,
                _ctx: &mut dyn Ctx,
                _offset: u64,
                _size: AccessSize,
                _value: u64,
                _attr: &Attr,
            ) -> Result<(), BusFault> {
                Ok(())
            }
        }
        impl EventTarget for LoopDevice {
            fn on_timer(&mut self, ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
                ctx.schedule_after(VTime::ZERO, 1)?;
                Ok(())
            }
        }
        let view =
            FlatView::build(&[leaf(0, 0x1000, DEVICE, MemoryKind::Mmio, Permissions::RW)]).unwrap();
        let mut machine = Machine::new(view, 128);
        machine
            .register_device(DEVICE, Box::new(LoopDevice))
            .unwrap();
        machine.schedule_timer(DEVICE, VTime::ZERO, 0).unwrap();
        assert_eq!(
            machine.run_until(VTime(100), 16),
            Err(MachineError::EventBudgetExceeded)
        );
    }

    #[test]
    fn idle_run_advances_time_and_unknown_timers_are_reported() {
        let (mut machine, _) = build_machine(8);
        assert_eq!(machine.now(), VTime::ZERO);
        assert_eq!(machine.run_until(VTime(400), 8).unwrap(), 0);
        assert_eq!(machine.now(), VTime(400));
        machine.schedule_timer(NodeId(99), VTime(500), 1).unwrap();
        assert!(matches!(
            machine.run_until(VTime(600), 8),
            Err(MachineError::NoTimerHandler(_))
        ));
    }

    #[test]
    fn identical_inputs_produce_identical_machine_traces() {
        fn scenario() -> (Vec<bool>, u64, QueueState) {
            let (mut machine, probe) = build_machine(64);
            let attr = bus_attr(RAM);
            machine
                .bus_write(Addr(0x2000), AccessSize::Word, 4, &attr)
                .unwrap();
            machine
                .bus_write(Addr(0x2004), AccessSize::Word, 2, &attr)
                .unwrap();
            machine
                .bus_write(Addr(0x1000), AccessSize::Word, 0xdead_beef, &attr)
                .unwrap();
            machine.run_until(VTime(1000), 64).unwrap();
            let observed = levels(&probe);
            let ram = machine
                .bus_read(Addr(0x1000), AccessSize::Word, &attr)
                .unwrap();
            (observed, ram, machine.save_queue())
        }
        assert_eq!(scenario(), scenario());
    }

    #[test]
    fn queue_checkpoint_replay_reproduces_the_machine_trace() {
        let (mut first, first_probe) = build_machine(64);
        let attr = bus_attr(RAM);
        first
            .bus_write(Addr(0x2000), AccessSize::Word, 3, &attr)
            .unwrap();
        let checkpoint = first.save_queue();
        first.run_until(VTime(1000), 64).unwrap();
        let expected = levels(&first_probe);

        let (mut restored, restored_probe) = build_machine(64);
        restored.load_queue(&checkpoint).unwrap();
        restored.run_until(VTime(1000), 64).unwrap();
        assert_eq!(levels(&restored_probe), expected);
        assert_eq!(restored.save_queue(), first.save_queue());
    }

    #[test]
    fn machine_keeps_the_send_contract() {
        fn assert_send<T: Send>() {}
        assert_send::<Machine>();
        assert_send::<Box<dyn MachineDevice>>();
    }

    #[test]
    #[ignore = "manual throughput probe; never part of deterministic CI output"]
    fn mmio_throughput_probe() {
        let (mut machine, _) = build_machine(4096);
        let attr = bus_attr(RAM);
        let rounds = 100_000u64;
        let start = std::time::Instant::now();
        for index in 0..rounds {
            machine
                .bus_write(Addr(0x1000), AccessSize::Word, index, &attr)
                .unwrap();
            let _ = machine
                .bus_read(Addr(0x1000), AccessSize::Word, &attr)
                .unwrap();
        }
        let elapsed = start.elapsed().as_secs_f64();
        let throughput = (rounds * 2) as f64 / elapsed;
        println!("ram mmio: {throughput:.0} ops/s ({rounds} read+write pairs in {elapsed:.3?})");
    }
}
