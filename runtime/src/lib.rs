#![forbid(unsafe_code)]
//! Deterministic runtime services. Host time and model references stay outside.
//!
//! The event queue is single-threaded. Events are delivered only by an explicit
//! caller-driven pop; scheduling an event or signal never invokes a model.
//! The [`machine::Machine`] composes the queue, the symbolic [`FlatView`], the
//! device arena and RAM/ROM backing into one MMIO and event dispatch path.

mod address;
mod arena;
mod backing;
mod clocktree;
mod context;
mod events;
mod machine;

pub use address::{Access, AddressError, FlatView, Region, Resolved};
pub use arena::DeviceArena;
pub use backing::{BackingBlockState, BackingState, BackingStore};
pub use clocktree::{
    ClockError, ClockId, ClockNodeState, ClockStructureNode, ClockTree, ClockTreeState,
};
pub use context::ModelContext;
pub use events::{Event, EventKind, EventQueue, QueueState};
pub use machine::{bus_attr, Machine, MachineDevice, MachineError, MachineSnapshot};
