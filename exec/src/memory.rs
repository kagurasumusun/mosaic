use crate::{Addr, NodeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl Permissions {
    pub const RWX: Self = Self {
        read: true,
        write: true,
        execute: true,
    };
    pub const RW: Self = Self {
        read: true,
        write: true,
        execute: false,
    };
    pub const RX: Self = Self {
        read: true,
        write: false,
        execute: true,
    };
    pub const R: Self = Self {
        read: true,
        write: false,
        execute: false,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryKind {
    Ram,
    Rom,
    Mmio,
}

/// A flattened address interval and its device-local offset, not a host pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mapping {
    pub base: Addr,
    pub size: u64,
    pub target: NodeId,
    pub offset: u64,
    pub kind: MemoryKind,
    pub permissions: Permissions,
}
