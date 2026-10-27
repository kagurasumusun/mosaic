#![forbid(unsafe_code)]
//! Typed, serialization-format-independent machine descriptions.
//!
//! This crate deliberately avoids external dependencies and has no runtime or
//! model implementation dependencies. Tooling parses external formats into
//! schema values, validates their semantics, and may then build a normalized
//! `MachineGraph`.

mod endpoint;
mod graph;
mod schema;

pub use graph::{GraphEdge, GraphNode, GraphNodeKind, GraphRegion, MachineGraph};
pub use schema::{
    AddressSpaceView, BlockInstance, BoardConnection, BoardIdentity, BoardManifest, ConfigValue,
    CpuInstance, DescriptorError, IrqRoute, PartInstance, Placement, RegionDescriptor, RegionKind,
    SocIdentity, SocManifest,
};

#[cfg(test)]
mod tests;
