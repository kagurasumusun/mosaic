//! Single-threaded machine: address resolution, MMIO dispatch and event delivery.
//!
//! The machine owns the symbolic [`FlatView`], the device arena, RAM/ROM
//! backing blocks, the deterministic event queue and the signal wiring table.
//! Models never reference each other: bus accesses resolve to a node id and
//! device-local offset, timer events reach nodes through the queue, and signal
//! changes are delivered only after the originating callback has returned.
//!
//! This is not yet a whole-machine emulator: CPU engine contexts, DMI host
//! pointers and a persistent codec are integrated later. The clock tree is
//! owned here and resolved by the composition root; catalog models still
//! receive a period rather than depending on this module.

use std::any::{type_name, Any, TypeId};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, EventTarget};
use mosaic_contract_signal::SignalSink;
use mosaic_core_types::{
    AccessKind, AccessSize, Addr, Attr, BusFault, Endian, EventId, Fnv64, MemoryKind, NodeId,
    Security, SignalId, Snapshot, SnapshotError, VTime,
};

use crate::address::{Access, FlatView, Resolved};
use crate::arena::DeviceArena;
use crate::backing::{BackingState, BackingStore};
use crate::clocktree::{self, ClockId, ClockTree, ClockTreeState};
use crate::context::ModelContext;
use crate::events::{Event, EventKind, EventQueue, QueueState};

/// Runtime composition of the contract facets a bus-driven model needs.
///
/// One boxed value serves both MMIO and timer callbacks, so register state is
/// never split between trait objects. The blanket impl keeps models writing
/// only the contract traits from `mosaic-contract-*`.
pub trait MachineDevice: MmioTarget + EventTarget {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: MmioTarget + EventTarget + 'static> MachineDevice for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub trait MachineSink: SignalSink {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: SignalSink + 'static> MachineSink for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Keeps a device registered even if a model callback unwinds.
///
/// MMIO callbacks temporarily take a device out of the arena to make reentrant
/// access structurally impossible. The lease restores it in `Drop`, including
/// during unwinding, so an accidental panic cannot permanently leave a valid
/// node in the arena's unavailable state.
struct DeviceLease<'a> {
    arena: &'a mut DeviceArena,
    node: NodeId,
    device: Option<Box<dyn MachineDevice>>,
}

impl<'a> DeviceLease<'a> {
    fn new(arena: &'a mut DeviceArena, node: NodeId, device: Box<dyn MachineDevice>) -> Self {
        Self {
            arena,
            node,
            device: Some(device),
        }
    }

    fn device_mut(&mut self) -> &mut dyn MachineDevice {
        self.device
            .as_deref_mut()
            .expect("device lease owns its device until Drop")
    }
}

impl Drop for DeviceLease<'_> {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            self.arena.restore(self.node, device);
        }
    }
}

const MACHINE_SNAPSHOT_SCHEMA: u32 = 2;

/// Exact runtime topology captured alongside its stable, non-cryptographic
/// fingerprint. The exact value comparison prevents a fingerprint collision
/// from accepting an incompatible in-memory checkpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
struct MachineTopology {
    default_view: FlatView,
    views: BTreeMap<u32, FlatView>,
    device_nodes: BTreeSet<u32>,
    sink_nodes: BTreeSet<u32>,
    backing_sizes: BTreeMap<u32, usize>,
    outputs: BTreeMap<u32, BTreeSet<u32>>,
    wiring: BTreeMap<(u32, u32), u32>,
    /// Clock-node structure only. Gate, mux and divisor values are state.
    clocks: Vec<clocktree::ClockStructureNode>,
}

impl MachineTopology {
    fn fingerprint(&self) -> u64 {
        let mut hash = Fnv64::new();
        hash.byte(2); // topology format version; 2 adds the clock-tree structure
        hash_view(&mut hash, &self.default_view);
        hash.usize(self.views.len());
        for (&master, view) in &self.views {
            hash.u32(master);
            hash_view(&mut hash, view);
        }
        hash_u32_set(&mut hash, &self.device_nodes);
        hash_u32_set(&mut hash, &self.sink_nodes);
        hash.usize(self.backing_sizes.len());
        for (&node, &size) in &self.backing_sizes {
            hash.u32(node);
            hash.usize(size);
        }
        hash.usize(self.outputs.len());
        for (&node, signals) in &self.outputs {
            hash.u32(node);
            hash_u32_set(&mut hash, signals);
        }
        hash.usize(self.wiring.len());
        for (&(source, signal), &sink) in &self.wiring {
            hash.u32(source);
            hash.u32(signal);
            hash.u32(sink);
        }
        clocktree::hash_structure(&mut hash, &self.clocks);
        hash.finish()
    }
}

fn hash_u32_set(hash: &mut Fnv64, values: &BTreeSet<u32>) {
    hash.usize(values.len());
    for value in values {
        hash.u32(*value);
    }
}

fn hash_view(hash: &mut Fnv64, view: &FlatView) {
    hash.usize(view.mappings().len());
    for mapping in view.mappings() {
        hash.u64(mapping.base.0);
        hash.u64(mapping.size);
        hash.u32(mapping.target.0);
        hash.u64(mapping.offset);
        hash.byte(match mapping.kind {
            MemoryKind::Ram => 0,
            MemoryKind::Rom => 1,
            MemoryKind::Mmio => 2,
        });
        hash.byte(u8::from(mapping.permissions.read));
        hash.byte(u8::from(mapping.permissions.write));
        hash.byte(u8::from(mapping.permissions.execute));
    }
}

/// Owned, in-memory checkpoint of the runtime portion of one Machine.
///
/// This includes the virtual timeline, RAM/ROM bytes, exact address/wiring
/// topology, and state for every device registered through
/// [`Machine::register_snapshot_device`]. It intentionally does not include
/// CPU engine contexts, shared-memory mirrors, signal sinks, host ports, or a
/// persistent byte encoding. `save_snapshot` rejects machines with devices or
/// sinks lacking snapshot support instead of silently producing a partial one.
pub struct MachineSnapshot {
    schema: u32,
    graph_fingerprint: u64,
    topology: MachineTopology,
    queue: QueueState,
    backing: BackingState,
    devices: BTreeMap<u32, ErasedDeviceState>,
    sinks: BTreeMap<u32, ErasedSinkState>,
    clocks: ClockTreeState,
    state_hash: u64,
}

impl MachineSnapshot {
    pub fn schema_version(&self) -> u32 {
        self.schema
    }

    /// Stable for an identical topology and independent of hash-randomization.
    /// This FNV-1a fingerprint is a compatibility checksum, not a security hash.
    pub fn graph_fingerprint(&self) -> u64 {
        self.graph_fingerprint
    }

    pub fn virtual_time(&self) -> VTime {
        self.queue.now
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn memory_bytes(&self) -> usize {
        self.backing
            .blocks
            .iter()
            .fold(0usize, |size, block| size.saturating_add(block.bytes.len()))
    }

    /// Deterministic digest of virtual time, events, RAM/ROM and device state.
    ///
    /// Topology is intentionally excluded: a replay of the same machine is
    /// compared by this value (設計書 §22 P4). It is an FNV-1a checksum, not
    /// a cryptographic hash and not a persistent-file identifier.
    pub fn state_hash(&self) -> u64 {
        self.state_hash
    }
}

impl fmt::Debug for MachineSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MachineSnapshot")
            .field("schema", &self.schema)
            .field(
                "graph_fingerprint",
                &format_args!("{:016x}", self.graph_fingerprint),
            )
            .field("virtual_time", &self.queue.now)
            .field("pending_events", &self.queue.events.len())
            .field("memory_bytes", &self.memory_bytes())
            .field("devices", &self.devices.keys().collect::<Vec<_>>())
            .finish()
    }
}

struct ErasedDeviceState {
    model: &'static str,
    device_type: TypeId,
    state_type: TypeId,
    version: u32,
    value: Box<dyn Any + Send>,
}

struct ErasedSinkState {
    model: &'static str,
    sink_type: TypeId,
    state_type: TypeId,
    version: u32,
    value: Box<dyn Any + Send>,
}

struct SinkSnapshotOps {
    model: &'static str,
    sink_type: TypeId,
    save: fn(&dyn MachineSink) -> Result<ErasedSinkState, SnapshotError>,
    validate: fn(&dyn MachineSink, &ErasedSinkState) -> Result<(), SnapshotError>,
    load: fn(&mut dyn MachineSink, &ErasedSinkState) -> Result<(), SnapshotError>,
    fingerprint: fn(&ErasedSinkState) -> Result<u64, SnapshotError>,
}

impl SinkSnapshotOps {
    fn for_type<T>() -> Self
    where
        T: MachineSink + Snapshot + 'static,
        T::State: 'static,
    {
        Self {
            model: type_name::<T>(),
            sink_type: TypeId::of::<T>(),
            save: save_sink::<T>,
            validate: validate_sink::<T>,
            load: load_sink::<T>,
            fingerprint: fingerprint_sink::<T>,
        }
    }
}

fn save_sink<T>(sink: &dyn MachineSink) -> Result<ErasedSinkState, SnapshotError>
where
    T: MachineSink + Snapshot + 'static,
    T::State: 'static,
{
    let sink = sink
        .as_any()
        .downcast_ref::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    Ok(ErasedSinkState {
        model: type_name::<T>(),
        sink_type: TypeId::of::<T>(),
        state_type: TypeId::of::<T::State>(),
        version: T::SCHEMA_VERSION,
        value: Box::new(sink.save()),
    })
}

fn validate_sink<T>(
    sink: &dyn MachineSink,
    erased: &ErasedSinkState,
) -> Result<(), SnapshotError>
where
    T: MachineSink + Snapshot + 'static,
    T::State: 'static,
{
    if erased.model != type_name::<T>()
        || erased.sink_type != TypeId::of::<T>()
        || erased.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let sink = sink
        .as_any()
        .downcast_ref::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    let state = erased
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    sink.validate(erased.version, state)
}

fn load_sink<T>(
    sink: &mut dyn MachineSink,
    state: &ErasedSinkState,
) -> Result<(), SnapshotError>
where
    T: MachineSink + Snapshot + 'static,
    T::State: 'static,
{
    if state.model != type_name::<T>()
        || state.sink_type != TypeId::of::<T>()
        || state.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let sink = sink
        .as_any_mut()
        .downcast_mut::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    let model_state = state
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    sink.load(state.version, model_state)
}

fn fingerprint_sink<T>(state: &ErasedSinkState) -> Result<u64, SnapshotError>
where
    T: SignalSink + Snapshot + 'static,
    T::State: 'static,
{
    if state.model != type_name::<T>()
        || state.sink_type != TypeId::of::<T>()
        || state.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let model_state = state
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    Ok(T::fingerprint(state.version, model_state))
}

struct DeviceSnapshotOps {
    model: &'static str,
    device_type: TypeId,
    save: fn(&dyn MachineDevice) -> Result<ErasedDeviceState, SnapshotError>,
    validate: fn(&dyn MachineDevice, &ErasedDeviceState) -> Result<(), SnapshotError>,
    load: fn(&mut dyn MachineDevice, &ErasedDeviceState) -> Result<(), SnapshotError>,
    fingerprint: fn(&ErasedDeviceState) -> Result<u64, SnapshotError>,
}

impl DeviceSnapshotOps {
    fn for_type<T>() -> Self
    where
        T: MachineDevice + Snapshot + 'static,
        T::State: 'static,
    {
        Self {
            model: type_name::<T>(),
            device_type: TypeId::of::<T>(),
            save: save_device::<T>,
            validate: validate_device::<T>,
            load: load_device::<T>,
            fingerprint: fingerprint_device::<T>,
        }
    }
}

fn save_device<T>(device: &dyn MachineDevice) -> Result<ErasedDeviceState, SnapshotError>
where
    T: MachineDevice + Snapshot + 'static,
    T::State: 'static,
{
    let device = device
        .as_any()
        .downcast_ref::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    Ok(ErasedDeviceState {
        model: type_name::<T>(),
        device_type: TypeId::of::<T>(),
        state_type: TypeId::of::<T::State>(),
        version: T::SCHEMA_VERSION,
        value: Box::new(device.save()),
    })
}

fn validate_device<T>(
    device: &dyn MachineDevice,
    erased: &ErasedDeviceState,
) -> Result<(), SnapshotError>
where
    T: MachineDevice + Snapshot + 'static,
    T::State: 'static,
{
    if erased.model != type_name::<T>()
        || erased.device_type != TypeId::of::<T>()
        || erased.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let device = device
        .as_any()
        .downcast_ref::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    let state = erased
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    device.validate(erased.version, state)
}

fn load_device<T>(
    device: &mut dyn MachineDevice,
    state: &ErasedDeviceState,
) -> Result<(), SnapshotError>
where
    T: MachineDevice + Snapshot + 'static,
    T::State: 'static,
{
    if state.model != type_name::<T>()
        || state.device_type != TypeId::of::<T>()
        || state.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let device = device
        .as_any_mut()
        .downcast_mut::<T>()
        .ok_or(SnapshotError::IdentityMismatch)?;
    let model_state = state
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    device.load(state.version, model_state)
}

fn fingerprint_device<T>(state: &ErasedDeviceState) -> Result<u64, SnapshotError>
where
    T: MachineDevice + Snapshot + 'static,
    T::State: 'static,
{
    if state.model != type_name::<T>()
        || state.device_type != TypeId::of::<T>()
        || state.state_type != TypeId::of::<T::State>()
    {
        return Err(SnapshotError::IdentityMismatch);
    }
    let model_state = state
        .value
        .downcast_ref::<T::State>()
        .ok_or(SnapshotError::InvalidState)?;
    Ok(T::fingerprint(state.version, model_state))
}

fn state_digest(
    queue: &QueueState,
    backing: &BackingState,
    devices: &BTreeMap<u32, ErasedDeviceState>,
    device_ops: &BTreeMap<u32, DeviceSnapshotOps>,
    sinks: &BTreeMap<u32, ErasedSinkState>,
    sink_ops: &BTreeMap<u32, SinkSnapshotOps>,
    clocks: &ClockTreeState,
) -> Result<u64, SnapshotError> {
    let mut hash = Fnv64::new();
    hash.byte(1);
    hash.u64(EventQueue::fingerprint(EventQueue::SCHEMA_VERSION, queue));
    hash.u64(BackingStore::fingerprint(
        BackingStore::SCHEMA_VERSION,
        backing,
    ));
    hash.usize(devices.len());
    for (node, state) in devices {
        let ops = device_ops.get(node).ok_or(SnapshotError::InvalidState)?;
        hash.u32(*node);
        hash.u32(state.version);
        hash.u64((ops.fingerprint)(state)?);
    }
    hash.usize(sinks.len());
    for (node, state) in sinks {
        let ops = sink_ops.get(node).ok_or(SnapshotError::InvalidState)?;
        hash.u32(*node);
        hash.u32(state.version);
        hash.u64((ops.fingerprint)(state)?);
    }
    hash.u64(ClockTree::fingerprint(ClockTree::SCHEMA_VERSION, clocks));
    Ok(hash.finish())
}

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
    /// One output port can drive only one sink in the current wiring model.
    DuplicateConnection {
        source: NodeId,
        signal: SignalId,
    },
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
    /// Per-master viewpoints (design §13.1: a FlatView per AddressSpace and
    /// observer). A bus transaction resolves through the view registered for
    /// its `Attr::master`, falling back to the machine's default view.
    views: BTreeMap<u32, FlatView>,
    arena: DeviceArena,
    sinks: BTreeMap<u32, Box<dyn MachineSink>>,
    backing: BackingStore,
    /// Per-node outgoing signal ids, derived from the wiring declarations.
    outputs: BTreeMap<u32, BTreeSet<u32>>,
    /// `(source node, signal id) -> sink node`.
    wiring: BTreeMap<(u32, u32), u32>,
    /// Snapshot vtables for typed devices. Devices without an entry make a
    /// whole-machine snapshot unavailable rather than silently omitting state.
    snapshot_devices: BTreeMap<u32, DeviceSnapshotOps>,
    snapshot_sinks: BTreeMap<u32, SinkSnapshotOps>,
    /// Frequency, divider, gate and mux tree. Models do not borrow it.
    clocks: ClockTree,
}

impl Machine {
    pub fn new(view: FlatView, queue_capacity: usize) -> Self {
        Self {
            queue: EventQueue::new(queue_capacity),
            view,
            views: BTreeMap::new(),
            arena: DeviceArena::new(),
            sinks: BTreeMap::new(),
            backing: BackingStore::new(),
            outputs: BTreeMap::new(),
            wiring: BTreeMap::new(),
            snapshot_devices: BTreeMap::new(),
            snapshot_sinks: BTreeMap::new(),
            clocks: ClockTree::new(),
        }
    }

    /// Clock tree used by the composition root to resolve instruction and
    /// timer periods. Adding or removing a node changes the graph fingerprint;
    /// gate, mux and divisor updates do not.
    pub fn clocks(&self) -> &ClockTree {
        &self.clocks
    }

    pub fn clocks_mut(&mut self) -> &mut ClockTree {
        &mut self.clocks
    }

    /// Resolved picosecond period of one tree node, for drivers that store a
    /// per-core period rather than consulting the tree on every slice.
    pub fn clock_period_ps(
        &self,
        clock: ClockId,
    ) -> Result<core::num::NonZeroU64, clocktree::ClockError> {
        self.clocks.period_ps(clock)
    }

    /// Register the viewpoint of one bus master (a CPU core or DMA master).
    /// Every master sees only its own registrations plus the default view;
    /// this is how per-core memory (boot overlays, TCM) is expressed without
    /// leaking another core's private ranges into a shared FlatView.
    pub fn add_view(&mut self, master: NodeId, view: FlatView) -> Result<(), MachineError> {
        if self.views.contains_key(&master.0) {
            return Err(MachineError::DuplicateNode(master));
        }
        self.views.insert(master.0, view);
        Ok(())
    }

    /// The view a transaction with `attr` resolves through.
    pub fn view_for(&self, attr: &Attr) -> &FlatView {
        self.views.get(&attr.master.0).unwrap_or(&self.view)
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
        if self.sinks.contains_key(&node.0) || self.backing.is_registered(node) {
            return Err(MachineError::DuplicateNode(node));
        }
        self.arena.insert(node, device)
    }

    /// Register a model whose typed State participates in whole-machine
    /// snapshots. Host ports remain owned by the model and must be excluded
    /// from its State so restoration preserves the current host connection.
    pub fn register_snapshot_device<T>(
        &mut self,
        node: NodeId,
        device: T,
    ) -> Result<(), MachineError>
    where
        T: MachineDevice + Snapshot + 'static,
        T::State: 'static,
    {
        if self.sinks.contains_key(&node.0) || self.backing.is_registered(node) {
            return Err(MachineError::DuplicateNode(node));
        }
        self.arena.insert(node, Box::new(device))?;
        self.snapshot_devices
            .insert(node.0, DeviceSnapshotOps::for_type::<T>());
        Ok(())
    }

    pub fn register_sink(
        &mut self,
        node: NodeId,
        sink: Box<dyn MachineSink>,
    ) -> Result<(), MachineError> {
        if self.sinks.contains_key(&node.0)
            || self.arena.is_registered(node)
            || self.backing.is_registered(node)
        {
            return Err(MachineError::DuplicateNode(node));
        }
        self.sinks.insert(node.0, sink);
        Ok(())
    }

    pub fn register_snapshot_sink<T>(
        &mut self,
        node: NodeId,
        sink: T,
    ) -> Result<(), MachineError>
    where
        T: MachineSink + Snapshot + 'static,
        T::State: 'static,
    {
        if self.sinks.contains_key(&node.0)
            || self.arena.is_registered(node)
            || self.backing.is_registered(node)
        {
            return Err(MachineError::DuplicateNode(node));
        }
        self.snapshot_sinks
            .insert(node.0, SinkSnapshotOps::for_type::<T>());
        self.sinks.insert(node.0, Box::new(sink));
        Ok(())
    }

    pub fn add_backing(&mut self, node: NodeId, size: u64) -> Result<(), MachineError> {
        // NodeId is the machine-wide identity across RAM/ROM, devices and
        // signal sinks; reusing it across registries makes topology ambiguous.
        if self.arena.is_registered(node) || self.sinks.contains_key(&node.0) {
            return Err(MachineError::DuplicateNode(node));
        }
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
        // A wired input lands on an arena device (single instance, Ctx-bound
        // delivery) or on a registered host sink node; anything else is a
        // wiring mistake that must fail at construction time.
        if !self.arena.is_registered(sink) && !self.sinks.contains_key(&sink.0) {
            return Err(MachineError::MissingSink(sink));
        }
        if self.wiring.contains_key(&(source.0, signal.0)) {
            return Err(MachineError::DuplicateConnection { source, signal });
        }
        self.outputs.entry(source.0).or_default().insert(signal.0);
        self.wiring.insert((source.0, signal.0), sink.0);
        Ok(())
    }

    /// Cross-check every registered view against the registered nodes: each
    /// Ram/Rom mapping needs a sufficiently large backing block, each Mmio
    /// mapping a device. Per-master viewpoints are checked like the default.
    pub fn verify(&self) -> Result<(), MachineError> {
        self.verify_view(&self.view)?;
        for view in self.views.values() {
            self.verify_view(view)?;
        }
        Ok(())
    }

    fn verify_view(&self, view: &FlatView) -> Result<(), MachineError> {
        for mapping in view.mappings() {
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
        // Resolve through the master's viewpoint first: the returned
        // resolution owns no borrow of the view, so the device dispatch below
        // can take &mut self again.
        let resolved = self.view_for(attr).resolve(address, size, Access::Read)?;
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
        let resolved = self.view_for(attr).resolve(address, size, Access::Write)?;
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
        let Some(device) = self.arena.take(node) else {
            return Err(self.absent_fault(node));
        };
        let mut lease = DeviceLease::new(&mut self.arena, node, device);
        let empty = BTreeSet::new();
        let outputs = self.outputs.get(&node.0).unwrap_or(&empty);
        let mut ctx = ModelContext::new(&mut self.queue, node, outputs);
        lease
            .device_mut()
            .read(&mut ctx, resolved.offset, size, attr)
    }

    fn dispatch_mmio_write(
        &mut self,
        resolved: Resolved,
        size: AccessSize,
        value: u64,
        attr: &Attr,
    ) -> Result<(), BusFault> {
        let node = resolved.target;
        let Some(device) = self.arena.take(node) else {
            return Err(self.absent_fault(node));
        };
        let mut lease = DeviceLease::new(&mut self.arena, node, device);
        let empty = BTreeSet::new();
        let outputs = self.outputs.get(&node.0).unwrap_or(&empty);
        let mut ctx = ModelContext::new(&mut self.queue, node, outputs);
        lease
            .device_mut()
            .write(&mut ctx, resolved.offset, size, value, attr)
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
            // Check the budget before popping. Popping first would silently
            // discard the next due event on the exact iteration that detects
            // exhaustion, making the machine impossible to resume faithfully.
            if delivered >= budget
                && self
                    .queue
                    .next_time()
                    .is_some_and(|event_time| event_time <= deadline)
            {
                return Err(MachineError::EventBudgetExceeded);
            }
            let Some(event) = self.queue.pop_due(deadline)? else {
                return Ok(delivered);
            };
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
                    let sink_node = NodeId(sink_node);
                    // Wired inputs land on the arena device when one owns the
                    // node: a single instance then sees both its MMIO face and
                    // its signal inputs, with a Ctx bound to its own outputs
                    // (aggregators such as interrupt controllers, §13.3/§13.4).
                    if let Some(device) = self.arena.event_target(sink_node) {
                        let outputs = self.outputs.get(&sink_node.0).cloned().unwrap_or_default();
                        let mut ctx = ModelContext::new(&mut self.queue, sink_node, &outputs);
                        device.on_signal(&mut ctx, owner, signal, high)?;
                    } else {
                        let Some(sink) = self.sinks.get_mut(&sink_node.0) else {
                            return Err(MachineError::MissingSink(sink_node));
                        };
                        sink.set_level(high);
                    }
                }
                // An unconnected output is a valid optional port: no destination.
            }
        }
        Ok(())
    }

    /// Capture the runtime state, refusing to omit unsnapshotable models or
    /// signal sinks. The resulting checkpoint is reusable in memory and can be
    /// loaded into another Machine with an exactly matching topology.
    pub fn save_snapshot(&self) -> Result<MachineSnapshot, SnapshotError> {
        let topology = self.snapshot_topology();
        let snapshot_nodes: BTreeSet<_> = self.snapshot_devices.keys().copied().collect();
        let snapshot_sink_nodes: BTreeSet<_> = self.snapshot_sinks.keys().copied().collect();
        if topology.sink_nodes != snapshot_sink_nodes || topology.device_nodes != snapshot_nodes {
            return Err(SnapshotError::CapabilityMissing);
        }
        if topology
            .device_nodes
            .iter()
            .any(|node| !self.arena.is_available(NodeId(*node)))
        {
            return Err(SnapshotError::InvalidState);
        }

        let mut devices = BTreeMap::new();
        for (&node, ops) in &self.snapshot_devices {
            let device = self
                .arena
                .get(NodeId(node))
                .ok_or(SnapshotError::InvalidState)?;
            if device.as_any().type_id() != ops.device_type {
                return Err(SnapshotError::IdentityMismatch);
            }
            let state = (ops.save)(device)?;
            if state.model != ops.model || state.device_type != ops.device_type {
                return Err(SnapshotError::IdentityMismatch);
            }
            (ops.validate)(device, &state)?;
            devices.insert(node, state);
        }

        let mut sinks = BTreeMap::new();
        for (&node, ops) in &self.snapshot_sinks {
            let sink = self
                .sinks
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            let state = (ops.save)(sink.as_ref())?;
            if state.model != ops.model || state.sink_type != ops.sink_type {
                return Err(SnapshotError::IdentityMismatch);
            }
            (ops.validate)(sink.as_ref(), &state)?;
            sinks.insert(node, state);
        }

        let queue = self.queue.save();
        validate_snapshot_events(&topology, &devices, &queue)?;
        let backing = self.backing.save();
        let clocks = self.clocks.save();
        let state_hash = state_digest(
            &queue,
            &backing,
            &devices,
            &self.snapshot_devices,
            &sinks,
            &self.snapshot_sinks,
            &clocks,
        )?;
        Ok(MachineSnapshot {
            schema: MACHINE_SNAPSHOT_SCHEMA,
            graph_fingerprint: topology.fingerprint(),
            topology,
            queue,
            backing,
            devices,
            sinks,
            clocks,
            state_hash,
        })
    }

    /// Restore a checkpoint after validating the graph, queue, backing blocks,
    /// and every device state. Device loads are prevalidated and rolled back if
    /// a device violates the Snapshot contract and rejects during application.
    pub fn load_snapshot(&mut self, snapshot: &MachineSnapshot) -> Result<(), SnapshotError> {
        if snapshot.schema != MACHINE_SNAPSHOT_SCHEMA {
            return Err(SnapshotError::VersionMismatch);
        }
        if snapshot.graph_fingerprint != snapshot.topology.fingerprint() {
            return Err(SnapshotError::InvalidState);
        }
        let topology = self.snapshot_topology();
        if snapshot.topology != topology || snapshot.graph_fingerprint != topology.fingerprint() {
            return Err(SnapshotError::GraphMismatch);
        }
        let snapshot_nodes: BTreeSet<_> = self.snapshot_devices.keys().copied().collect();
        let snapshot_sink_nodes: BTreeSet<_> = self.snapshot_sinks.keys().copied().collect();
        if topology.sink_nodes != snapshot_sink_nodes || topology.device_nodes != snapshot_nodes {
            return Err(SnapshotError::CapabilityMissing);
        }
        let state_nodes: BTreeSet<_> = snapshot.devices.keys().copied().collect();
        if state_nodes != snapshot_nodes {
            return Err(SnapshotError::InvalidState);
        }
        let state_sink_nodes: BTreeSet<_> = snapshot.sinks.keys().copied().collect();
        if state_sink_nodes != snapshot_sink_nodes {
            return Err(SnapshotError::InvalidState);
        }

        self.queue
            .validate(EventQueue::SCHEMA_VERSION, &snapshot.queue)?;
        self.backing
            .validate(BackingStore::SCHEMA_VERSION, &snapshot.backing)?;
        self.clocks
            .validate(ClockTree::SCHEMA_VERSION, &snapshot.clocks)?;
        validate_snapshot_events(&topology, &snapshot.devices, &snapshot.queue)?;

        // Validate every participant and capture a rollback state before the
        // first mutation. Snapshot::load itself must be atomic on failure.
        let mut backups = BTreeMap::new();
        for (&node, ops) in &self.snapshot_devices {
            let device = self
                .arena
                .get(NodeId(node))
                .ok_or(SnapshotError::InvalidState)?;
            if device.as_any().type_id() != ops.device_type {
                return Err(SnapshotError::IdentityMismatch);
            }
            let state = snapshot
                .devices
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            (ops.validate)(device, state)?;
            backups.insert(node, (ops.save)(device)?);
        }
        let mut sink_backups = BTreeMap::new();
        for (&node, ops) in &self.snapshot_sinks {
            let sink = self
                .sinks
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            let state = snapshot
                .sinks
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            (ops.validate)(sink.as_ref(), state)?;
            sink_backups.insert(node, (ops.save)(sink.as_ref())?);
        }
        let digest = state_digest(
            &snapshot.queue,
            &snapshot.backing,
            &snapshot.devices,
            &self.snapshot_devices,
            &snapshot.sinks,
            &self.snapshot_sinks,
            &snapshot.clocks,
        )?;
        if digest != snapshot.state_hash {
            return Err(SnapshotError::InvalidState);
        }

        let mut applied = Vec::new();
        for (&node, ops) in &self.snapshot_devices {
            let state = snapshot
                .devices
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            let device = self
                .arena
                .get_mut(NodeId(node))
                .ok_or(SnapshotError::InvalidState)?;
            applied.push(node);
            if let Err(error) = (ops.load)(device, state) {
                if !rollback_device_states(
                    &mut self.arena,
                    &self.snapshot_devices,
                    &backups,
                    &applied,
                ) {
                    return Err(SnapshotError::InvalidState);
                }
                return Err(error);
            }
        }

        let mut sink_applied = Vec::new();
        for (&node, ops) in &self.snapshot_sinks {
            let state = snapshot
                .sinks
                .get(&node)
                .ok_or(SnapshotError::InvalidState)?;
            let sink = self
                .sinks
                .get_mut(&node)
                .ok_or(SnapshotError::InvalidState)?;
            sink_applied.push(node);
            if let Err(error) = (ops.load)(sink.as_mut(), state) {
                let _ = rollback_device_states(
                    &mut self.arena,
                    &self.snapshot_devices,
                    &backups,
                    &applied,
                );
                let _ = rollback_sink_states(
                    &mut self.sinks,
                    &self.snapshot_sinks,
                    &sink_backups,
                    &sink_applied,
                );
                return Err(error);
            }
        }

        // Clock dynamics were validated above. Apply them before the queue and
        // RAM commits so a contract violation still rolls the devices back.
        if let Err(error) = self.clocks.apply_dynamics(&snapshot.clocks) {
            let applied: Vec<u32> = self.snapshot_devices.keys().copied().collect();
            let _ = rollback_device_states(&mut self.arena, &self.snapshot_devices, &backups, &applied);
            let sink_applied: Vec<u32> = self.snapshot_sinks.keys().copied().collect();
            let _ = rollback_sink_states(&mut self.sinks, &self.snapshot_sinks, &sink_backups, &sink_applied);
            return Err(SnapshotError::from(error));
        }
        self.backing.restore_validated(&snapshot.backing);
        self.queue.restore_validated(&snapshot.queue);
        Ok(())
    }

    fn snapshot_topology(&self) -> MachineTopology {
        MachineTopology {
            default_view: self.view.clone(),
            views: self.views.clone(),
            device_nodes: self.arena.node_ids().map(|node| node.0).collect(),
            sink_nodes: self.sinks.keys().copied().collect(),
            backing_sizes: self.backing.block_sizes(),
            outputs: self.outputs.clone(),
            wiring: self.wiring.clone(),
            clocks: self.clocks.structure(),
        }
    }

    pub fn graph_fingerprint(&self) -> u64 {
        self.snapshot_topology().fingerprint()
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

fn validate_snapshot_events(
    topology: &MachineTopology,
    devices: &BTreeMap<u32, ErasedDeviceState>,
    queue: &QueueState,
) -> Result<(), SnapshotError> {
    for event in &queue.events {
        if !devices.contains_key(&event.owner.0) {
            return Err(SnapshotError::InvalidState);
        }
        if let EventKind::Signal { signal, .. } = event.kind {
            if !topology
                .outputs
                .get(&event.owner.0)
                .is_some_and(|signals| signals.contains(&signal.0))
            {
                return Err(SnapshotError::InvalidState);
            }
        }
    }
    Ok(())
}

fn rollback_device_states(
    arena: &mut DeviceArena,
    ops_by_node: &BTreeMap<u32, DeviceSnapshotOps>,
    backups: &BTreeMap<u32, ErasedDeviceState>,
    applied: &[u32],
) -> bool {
    for &node in applied.iter().rev() {
        let (Some(ops), Some(state), Some(device)) = (
            ops_by_node.get(&node),
            backups.get(&node),
            arena.get_mut(NodeId(node)),
        ) else {
            return false;
        };
        if (ops.load)(device, state).is_err() {
            return false;
        }
    }
    true
}

fn rollback_sink_states(
    sinks: &mut BTreeMap<u32, Box<dyn MachineSink>>,
    ops_by_node: &BTreeMap<u32, SinkSnapshotOps>,
    backups: &BTreeMap<u32, ErasedSinkState>,
    applied: &[u32],
) -> bool {
    for &node in applied.iter().rev() {
        let (Some(ops), Some(state), Some(sink)) = (
            ops_by_node.get(&node),
            backups.get(&node),
            sinks.get_mut(&node),
        ) else {
            return false;
        };
        if (ops.load)(sink.as_mut(), state).is_err() {
            return false;
        }
    }
    true
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
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use mosaic_contract_model::Ctx;
    use mosaic_core_types::{Mapping, Permissions, VDelta};

    // Region constructors live in the runtime address module.
    use crate::address::Region;

    const ROM: NodeId = NodeId(1);
    const RAM: NodeId = NodeId(2);
    const DEVICE: NodeId = NodeId(3);
    const SINK: NodeId = NodeId(4);

    struct PanicOnceDevice {
        should_panic: Arc<AtomicBool>,
    }

    impl MmioTarget for PanicOnceDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            if self.should_panic.swap(false, Ordering::SeqCst) {
                panic!("intentional test panic in MMIO callback");
            }
            Ok(0x5A)
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

    impl EventTarget for PanicOnceDevice {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }
    }

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
            ctx.schedule_after(VDelta(value.saturating_mul(100)), offset)
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

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct SnapshotProbeState {
        schema: u32,
        value: u64,
        ticks: u64,
    }

    struct SnapshotProbe {
        value: u64,
        ticks: u64,
    }

    impl SnapshotProbe {
        fn new() -> Self {
            Self { value: 0, ticks: 0 }
        }
    }

    impl MmioTarget for SnapshotProbe {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            match offset {
                0 => Ok(self.value),
                4 => Ok(self.ticks),
                _ => Err(BusFault::Device),
            }
        }

        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            offset: u64,
            _size: AccessSize,
            value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            if offset != 0 {
                return Err(BusFault::Device);
            }
            self.value = value;
            Ok(())
        }
    }

    impl EventTarget for SnapshotProbe {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, token: u64) -> Result<(), ContextError> {
            self.ticks += token;
            Ok(())
        }
    }

    impl Snapshot for SnapshotProbe {
        type State = SnapshotProbeState;
        const SCHEMA_VERSION: u32 = 1;

        fn save(&self) -> Self::State {
            SnapshotProbeState {
                schema: 1,
                value: self.value,
                ticks: self.ticks,
            }
        }

        fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
            if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
                return Err(SnapshotError::VersionMismatch);
            }
            Ok(())
        }

        fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
            self.validate(version, state)?;
            self.value = state.value;
            self.ticks = state.ticks;
            Ok(())
        }

        fn fingerprint(version: u32, state: &Self::State) -> u64 {
            let mut hash = mosaic_core_types::Fnv64::new();
            hash.u32(version);
            hash.u32(state.schema);
            hash.u64(state.value);
            hash.u64(state.ticks);
            hash.finish()
        }
    }

    /// Test participant that deliberately mutates before rejecting one state,
    /// exercising the aggregate rollback path against a misbehaving model.
    struct MutatingLoadDevice {
        value: u64,
    }

    impl MmioTarget for MutatingLoadDevice {
        fn read(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            _attr: &Attr,
        ) -> Result<u64, BusFault> {
            Ok(self.value)
        }

        fn write(
            &mut self,
            _ctx: &mut dyn Ctx,
            _offset: u64,
            _size: AccessSize,
            value: u64,
            _attr: &Attr,
        ) -> Result<(), BusFault> {
            self.value = value;
            Ok(())
        }
    }

    impl EventTarget for MutatingLoadDevice {
        fn on_timer(&mut self, _ctx: &mut dyn Ctx, _token: u64) -> Result<(), ContextError> {
            Ok(())
        }
    }

    impl Snapshot for MutatingLoadDevice {
        type State = u64;
        const SCHEMA_VERSION: u32 = 1;

        fn save(&self) -> Self::State {
            self.value
        }

        fn validate(&self, version: u32, _state: &Self::State) -> Result<(), SnapshotError> {
            if version != Self::SCHEMA_VERSION {
                return Err(SnapshotError::VersionMismatch);
            }
            Ok(())
        }

        fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
            self.validate(version, state)?;
            self.value = *state;
            if *state == 42 {
                return Err(SnapshotError::InvalidState);
            }
            Ok(())
        }

        fn fingerprint(version: u32, state: &Self::State) -> u64 {
            let mut hash = mosaic_core_types::Fnv64::new();
            hash.u32(version);
            hash.u64(*state);
            hash.finish()
        }
    }

    #[test]
    fn snapshot_rollback_restores_the_device_that_rejected_load() {
        let view = FlatView::build(&[leaf(0x2000, 16, DEVICE, MemoryKind::Mmio, Permissions::RW)])
            .unwrap();
        let mut machine = Machine::new(view, 8);
        machine
            .register_snapshot_device(DEVICE, MutatingLoadDevice { value: 42 })
            .unwrap();
        let checkpoint = machine.save_snapshot().unwrap();

        machine
            .bus_write(Addr(0x2000), AccessSize::Word, 7, &bus_attr(NodeId(9)))
            .unwrap();
        assert_eq!(
            machine.load_snapshot(&checkpoint),
            Err(SnapshotError::InvalidState)
        );
        // The failing participant mutated itself to 42 before returning Err;
        // the aggregate must restore the pre-load value of 7 as well.
        assert_eq!(
            machine.bus_read(Addr(0x2000), AccessSize::Word, &bus_attr(NodeId(9))),
            Ok(7)
        );
    }

    fn snapshot_probe_machine(ram_size: u64) -> Machine {
        let ram = Region::leaf(
            Mapping {
                base: Addr(0x1000),
                size: ram_size,
                target: RAM,
                offset: 0,
                kind: MemoryKind::Ram,
                permissions: Permissions::RWX,
            },
            0,
        );
        let device = Region::leaf(
            Mapping {
                base: Addr(0x2000),
                size: 16,
                target: DEVICE,
                offset: 0,
                kind: MemoryKind::Mmio,
                permissions: Permissions::RW,
            },
            0,
        );
        let view = FlatView::build(&[ram, device]).unwrap();
        let mut machine = Machine::new(view, 8);
        machine.add_backing(RAM, ram_size).unwrap();
        machine
            .register_snapshot_device(DEVICE, SnapshotProbe::new())
            .unwrap();
        machine.verify().unwrap();
        machine
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
    fn panicking_mmio_callback_does_not_remove_the_device_from_the_arena() {
        let view = FlatView::build(&[leaf(
            0x2000,
            0x1000,
            DEVICE,
            MemoryKind::Mmio,
            Permissions::RW,
        )])
        .unwrap();
        let should_panic = Arc::new(AtomicBool::new(true));
        let mut machine = Machine::new(view, 8);
        machine
            .register_device(
                DEVICE,
                Box::new(PanicOnceDevice {
                    should_panic: Arc::clone(&should_panic),
                }),
            )
            .unwrap();

        let first = catch_unwind(AssertUnwindSafe(|| {
            machine.bus_read(Addr(0x2000), AccessSize::Word, &bus_attr(NodeId(9)))
        }));
        assert!(first.is_err());
        assert_eq!(
            machine.bus_read(Addr(0x2000), AccessSize::Word, &bus_attr(NodeId(9))),
            Ok(0x5A)
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
    fn wired_inputs_reach_arena_devices_with_source_identity() {
        // Two producing devices wired into one aggregating arena device: the
        // aggregator must tell them apart by (node, signal) and may re-emit
        // its own aggregated output through the delivery Ctx, like a VIC
        // aggregating IRQ lines (構成設計書 §13.3 / §13.4 rule 1).
        #[derive(Default)]
        struct AggregatorDevice {
            seen: Arc<Mutex<Vec<(u32, u32, bool)>>>,
            output: Option<SignalId>,
        }
        impl MmioTarget for AggregatorDevice {
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
        impl mosaic_contract_model::EventTarget for AggregatorDevice {
            fn on_timer(
                &mut self,
                _ctx: &mut dyn Ctx,
                _token: u64,
            ) -> Result<(), mosaic_contract_model::ContextError> {
                Ok(())
            }
            fn on_signal(
                &mut self,
                ctx: &mut dyn Ctx,
                source: NodeId,
                signal: SignalId,
                high: bool,
            ) -> Result<(), mosaic_contract_model::ContextError> {
                self.seen
                    .lock()
                    .expect("probe lock")
                    .push((source.0, signal.0, high));
                // Aggregated output: wired onward, so the set_signal must land.
                if let Some(output) = self.output {
                    ctx.set_signal(output, high)?;
                }
                Ok(())
            }
        }

        #[derive(Default)]
        struct LevelRecorder {
            levels: Arc<Mutex<Vec<bool>>>,
        }
        impl SignalSink for LevelRecorder {
            fn set_level(&mut self, high: bool) {
                self.levels.lock().expect("recorder lock").push(high);
            }
        }

        #[derive(Default)]
        struct EmitDevice {
            output: Option<SignalId>,
        }
        impl MmioTarget for EmitDevice {
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
                ctx: &mut dyn Ctx,
                _offset: u64,
                _size: AccessSize,
                value: u64,
                _attr: &Attr,
            ) -> Result<(), BusFault> {
                let Some(output) = self.output else {
                    return Ok(());
                };
                ctx.set_signal(output, value & 1 == 1)
                    .map_err(|_| BusFault::Device)
            }
        }
        impl mosaic_contract_model::EventTarget for EmitDevice {
            fn on_timer(
                &mut self,
                _ctx: &mut dyn Ctx,
                _token: u64,
            ) -> Result<(), mosaic_contract_model::ContextError> {
                Ok(())
            }
        }

        let first = NodeId(10);
        let second = NodeId(11);
        let aggregator = NodeId(12);
        let final_node = NodeId(13);
        let aggregated_out = SignalId(7);
        let view = FlatView::build(&[
            leaf(0x2000, 0x1000, first, MemoryKind::Mmio, Permissions::RW),
            leaf(0x3000, 0x1000, second, MemoryKind::Mmio, Permissions::RW),
        ])
        .unwrap();
        let mut machine = Machine::new(view, 8);
        machine
            .register_device(
                first,
                Box::new(EmitDevice {
                    output: Some(SignalId(2)),
                }),
            )
            .unwrap();
        machine
            .register_device(
                second,
                Box::new(EmitDevice {
                    output: Some(SignalId(1)),
                }),
            )
            .unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        // One instance, two faces: MMIO target and signal aggregator.
        machine
            .register_device(
                aggregator,
                Box::new(AggregatorDevice {
                    seen: Arc::clone(&seen),
                    output: Some(aggregated_out),
                }),
            )
            .unwrap();
        let levels = Arc::new(Mutex::new(Vec::new()));
        machine
            .register_sink(
                final_node,
                Box::new(LevelRecorder {
                    levels: Arc::clone(&levels),
                }),
            )
            .unwrap();
        machine.connect(first, SignalId(2), aggregator).unwrap();
        machine.connect(second, SignalId(1), aggregator).unwrap();
        // The aggregator's own output is wired like any device output.
        machine
            .connect(aggregator, aggregated_out, final_node)
            .unwrap();
        machine.verify().unwrap();

        let attr = bus_attr(first);
        machine
            .bus_write(Addr(0x2000), AccessSize::Word, 1, &attr)
            .unwrap();
        machine
            .bus_write(Addr(0x3000), AccessSize::Word, 1, &attr)
            .unwrap();
        machine.run_until(VTime(1), 8).unwrap();
        assert_eq!(
            *seen.lock().expect("probe lock"),
            vec![(first.0, 2, true), (second.0, 1, true)]
        );
        assert_eq!(*levels.lock().expect("recorder lock"), vec![true, true]);
    }

    #[test]
    fn construction_and_verification_reject_broken_wiring() {
        let (mut machine, _) = build_machine(8);
        assert_eq!(
            machine.register_device(DEVICE, Box::new(PacingDevice::default())),
            Err(MachineError::DuplicateNode(DEVICE))
        );
        assert_eq!(
            machine.register_sink(DEVICE, Box::new(LevelProbe::default())),
            Err(MachineError::DuplicateNode(DEVICE))
        );
        assert_eq!(
            machine.register_device(SINK, Box::new(PacingDevice::default())),
            Err(MachineError::DuplicateNode(SINK))
        );
        assert_eq!(
            machine.connect(DEVICE, SignalId(1), NodeId(99)),
            Err(MachineError::MissingSink(NodeId(99)))
        );
        assert_eq!(
            machine.connect(DEVICE, SignalId(1), SINK),
            Err(MachineError::DuplicateConnection {
                source: DEVICE,
                signal: SignalId(1)
            })
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
            machine.add_backing(DEVICE, 0x100),
            Err(MachineError::DuplicateNode(DEVICE))
        );
        assert_eq!(
            machine.add_backing(SINK, 0x100),
            Err(MachineError::DuplicateNode(SINK))
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
                ctx.schedule_after(VDelta(0), 1)?;
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
        // The event scheduled by the 16th callback remains pending. The
        // budget-exhaustion path must not pop-and-drop the 17th event.
        assert_eq!(machine.pending_events(), 1);
        assert_eq!(machine.now(), VTime::ZERO);
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
    fn full_runtime_snapshot_restores_memory_device_state_and_events_reusably() {
        let mut first = snapshot_probe_machine(16);
        first.loader_write(RAM, 0, &[0xAA, 0xBB]).unwrap();
        first
            .bus_write(Addr(0x2000), AccessSize::Word, 41, &bus_attr(NodeId(9)))
            .unwrap();
        first.schedule_timer(DEVICE, VTime(20), 7).unwrap();
        let checkpoint = first.save_snapshot().unwrap();
        assert_eq!(checkpoint.schema_version(), 2);
        assert_ne!(checkpoint.state_hash(), 0);
        assert_eq!(checkpoint.device_count(), 1);
        assert_eq!(checkpoint.memory_bytes(), 16);
        assert_ne!(checkpoint.graph_fingerprint(), 0);

        first.loader_write(RAM, 0, &[0xCC, 0xDD]).unwrap();
        first
            .bus_write(Addr(0x2000), AccessSize::Word, 99, &bus_attr(NodeId(9)))
            .unwrap();
        first.run_until(VTime(20), 8).unwrap();
        assert_eq!(first.now(), VTime(20));

        first.load_snapshot(&checkpoint).unwrap();
        assert_eq!(first.now(), VTime::ZERO);
        assert_eq!(first.pending_events(), 1);
        let mut ram = [0; 2];
        first.loader_read(RAM, 0, &mut ram).unwrap();
        assert_eq!(ram, [0xAA, 0xBB]);
        let attr = bus_attr(NodeId(9));
        assert_eq!(
            first.bus_read(Addr(0x2000), AccessSize::Word, &attr),
            Ok(41)
        );
        assert_eq!(first.bus_read(Addr(0x2004), AccessSize::Word, &attr), Ok(0));
        first.run_until(VTime(20), 8).unwrap();
        assert_eq!(first.bus_read(Addr(0x2004), AccessSize::Word, &attr), Ok(7));

        // The typed checkpoint can be restored into another instance and can
        // be replayed more than once; EventIds and insertion order are restored.
        let mut second = snapshot_probe_machine(16);
        second.load_snapshot(&checkpoint).unwrap();
        assert_eq!(second.graph_fingerprint(), checkpoint.graph_fingerprint());
        second.run_until(VTime(20), 8).unwrap();
        assert_eq!(
            second.bus_read(Addr(0x2000), AccessSize::Word, &attr),
            Ok(41)
        );
        assert_eq!(
            second.bus_read(Addr(0x2004), AccessSize::Word, &attr),
            Ok(7)
        );
        assert_eq!(second.now(), VTime(20));

        first.load_snapshot(&checkpoint).unwrap();
        first.run_until(VTime(20), 8).unwrap();
        assert_eq!(first.bus_read(Addr(0x2004), AccessSize::Word, &attr), Ok(7));
        assert_eq!(
            first.save_snapshot().unwrap().state_hash(),
            second.save_snapshot().unwrap().state_hash()
        );
    }

    #[test]
    fn save_restore_replay_keeps_the_state_hash_and_clock_gates() {
        let mut machine = snapshot_probe_machine(16);
        let source = machine
            .clocks_mut()
            .add_source(core::num::NonZeroU64::new(1_000_000_000).unwrap());
        let gate = machine.clocks_mut().add_gate(source, true).unwrap();
        machine.loader_write(RAM, 0, &[0x11]).unwrap();
        machine
            .bus_write(Addr(0x2000), AccessSize::Word, 5, &bus_attr(NodeId(9)))
            .unwrap();
        machine.schedule_timer(DEVICE, VTime(20), 3).unwrap();
        let checkpoint = machine.save_snapshot().unwrap();
        let before = checkpoint.state_hash();

        machine.clocks_mut().set_enabled(gate, false).unwrap();
        machine.loader_write(RAM, 0, &[0x22]).unwrap();
        machine.run_until(VTime(20), 8).unwrap();
        assert_ne!(machine.save_snapshot().unwrap().state_hash(), before);

        machine.load_snapshot(&checkpoint).unwrap();
        assert_eq!(machine.save_snapshot().unwrap().state_hash(), before);
        assert!(matches!(
            machine.clocks().resolve(gate).unwrap(),
            mosaic_contract_clock::ClockState::Running { .. }
        ));
        machine.run_until(VTime(20), 8).unwrap();
        let replayed = machine.save_snapshot().unwrap().state_hash();

        let mut other = snapshot_probe_machine(16);
        let other_source = other
            .clocks_mut()
            .add_source(core::num::NonZeroU64::new(1_000_000_000).unwrap());
        other.clocks_mut().add_gate(other_source, true).unwrap();
        other.load_snapshot(&checkpoint).unwrap();
        other.run_until(VTime(20), 8).unwrap();
        assert_eq!(other.save_snapshot().unwrap().state_hash(), replayed);

        let mut different = snapshot_probe_machine(16);
        different
            .clocks_mut()
            .add_source(core::num::NonZeroU64::new(8_000_000).unwrap());
        assert_eq!(
            different.load_snapshot(&checkpoint),
            Err(SnapshotError::GraphMismatch)
        );
    }

    #[test]
    fn machine_snapshot_prevalidates_every_device_before_mutating() {
        let mut source = snapshot_probe_machine(16);
        source.loader_write(RAM, 0, &[0xAA]).unwrap();
        source
            .bus_write(Addr(0x2000), AccessSize::Word, 41, &bus_attr(NodeId(9)))
            .unwrap();
        let mut checkpoint = source.save_snapshot().unwrap();
        checkpoint
            .devices
            .get_mut(&DEVICE.0)
            .unwrap()
            .value
            .downcast_mut::<SnapshotProbeState>()
            .unwrap()
            .schema = 2;

        let mut destination = snapshot_probe_machine(16);
        destination.loader_write(RAM, 0, &[0x55]).unwrap();
        destination
            .bus_write(Addr(0x2000), AccessSize::Word, 99, &bus_attr(NodeId(9)))
            .unwrap();
        assert_eq!(
            destination.load_snapshot(&checkpoint),
            Err(SnapshotError::VersionMismatch)
        );
        let mut byte = [0];
        destination.loader_read(RAM, 0, &mut byte).unwrap();
        assert_eq!(byte, [0x55]);
        assert_eq!(
            destination.bus_read(Addr(0x2000), AccessSize::Word, &bus_attr(NodeId(9))),
            Ok(99)
        );
    }

    #[test]
    fn machine_snapshot_rejects_graph_mismatch_without_mutation() {
        let mut source = snapshot_probe_machine(16);
        source.loader_write(RAM, 0, &[0xAA]).unwrap();
        let checkpoint = source.save_snapshot().unwrap();

        let mut incompatible = snapshot_probe_machine(32);
        incompatible.loader_write(RAM, 0, &[0x55]).unwrap();
        assert_eq!(
            incompatible.load_snapshot(&checkpoint),
            Err(SnapshotError::GraphMismatch)
        );
        let mut byte = [0];
        incompatible.loader_read(RAM, 0, &mut byte).unwrap();
        assert_eq!(byte, [0x55]);
    }

    #[test]
    fn machine_snapshot_refuses_devices_without_snapshot_support() {
        let mut machine = Machine::new(FlatView::build(&[]).unwrap(), 8);
        machine
            .register_device(DEVICE, Box::new(PacingDevice::default()))
            .unwrap();
        assert_eq!(
            machine.save_snapshot().unwrap_err(),
            SnapshotError::CapabilityMissing
        );
    }

    #[test]
    fn per_master_views_resolve_the_same_address_independently() {
        const ROM_A: NodeId = NodeId(5);
        const ROM_B: NodeId = NodeId(6);
        const OTHER: NodeId = NodeId(7);
        let default_view =
            FlatView::build(&[leaf(0, 0x1000, ROM_A, MemoryKind::Ram, Permissions::RW)]).unwrap();
        let other_view =
            FlatView::build(&[leaf(0, 0x1000, ROM_B, MemoryKind::Ram, Permissions::RW)]).unwrap();
        let mut machine = Machine::new(default_view, 8);
        machine.add_view(OTHER, other_view).unwrap();
        machine.add_backing(ROM_A, 0x1000).unwrap();
        machine.add_backing(ROM_B, 0x1000).unwrap();
        machine.verify().unwrap();
        assert_eq!(
            machine.add_view(OTHER, FlatView::build(&[]).unwrap()),
            Err(MachineError::DuplicateNode(OTHER))
        );
        machine.loader_write(ROM_A, 0, &[0xAA]).unwrap();
        machine.loader_write(ROM_B, 0, &[0xBB]).unwrap();
        let default_master = bus_attr(NodeId(0));
        let other_master = bus_attr(OTHER);
        // The same address resolves to a different backing per viewpoint:
        // per-core boot images without a shared alias (design §13.1).
        assert_eq!(
            machine.bus_read(Addr(0), AccessSize::Byte, &default_master),
            Ok(0xAA)
        );
        assert_eq!(
            machine.bus_read(Addr(0), AccessSize::Byte, &other_master),
            Ok(0xBB)
        );
        machine
            .bus_write(Addr(0), AccessSize::Byte, 1, &other_master)
            .unwrap();
        assert_eq!(
            machine.bus_read(Addr(0), AccessSize::Byte, &default_master),
            Ok(0xAA)
        );
    }

    #[test]
    fn verify_checks_per_master_views() {
        let broken =
            FlatView::build(&[leaf(0, 0x1000, NodeId(8), MemoryKind::Ram, Permissions::RW)])
                .unwrap();
        let mut machine = Machine::new(FlatView::build(&[]).unwrap(), 8);
        machine.add_view(NodeId(9), broken).unwrap();
        assert!(matches!(
            machine.verify(),
            Err(MachineError::UnknownNode(NodeId(8)))
        ));
    }

    #[test]
    fn snapshot_restores_snapshot_capable_sinks() {
        #[derive(Clone, Debug, Eq, PartialEq)]
        struct ProbeSinkState {
            schema: u32,
            level: bool,
        }

        struct ProbeSink {
            level: bool,
        }

        impl SignalSink for ProbeSink {
            fn set_level(&mut self, high: bool) {
                self.level = high;
            }
        }

        impl Snapshot for ProbeSink {
            type State = ProbeSinkState;
            const SCHEMA_VERSION: u32 = 1;

            fn save(&self) -> Self::State {
                ProbeSinkState {
                    schema: 1,
                    level: self.level,
                }
            }

            fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
                if version != Self::SCHEMA_VERSION || state.schema != Self::SCHEMA_VERSION {
                    return Err(SnapshotError::VersionMismatch);
                }
                Ok(())
            }

            fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError> {
                self.validate(version, state)?;
                self.level = state.level;
                Ok(())
            }

            fn fingerprint(version: u32, state: &Self::State) -> u64 {
                let mut hash = Fnv64::new();
                hash.u32(version);
                hash.bool(state.level);
                hash.finish()
            }
        }

        let view = FlatView::build(&[leaf(
            0x2000,
            16,
            DEVICE,
            MemoryKind::Mmio,
            Permissions::RW,
        )])
        .unwrap();
        let mut machine = Machine::new(view, 8);
        machine
            .register_snapshot_device(DEVICE, SnapshotProbe::new())
            .unwrap();
        machine
            .register_snapshot_sink(SINK, ProbeSink { level: false })
            .unwrap();
        machine.connect(DEVICE, SignalId(1), SINK).unwrap();

        let checkpoint = machine.save_snapshot().unwrap();
        assert_eq!(checkpoint.schema_version(), 2);

        machine.load_snapshot(&checkpoint).unwrap();
        assert_eq!(
            machine.save_snapshot().unwrap().state_hash(),
            checkpoint.state_hash()
        );
    }

    #[test]
    fn machine_keeps_the_send_contract() {
        fn assert_send<T: Send>() {}
        assert_send::<Machine>();
        assert_send::<MachineSnapshot>();
        assert_send::<Box<dyn MachineDevice>>();
        assert_send::<Box<dyn MachineSink>>();
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
