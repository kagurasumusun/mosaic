#![no_std]
#![forbid(unsafe_code)]
//! Shared VM vocabulary. No runtime or host services belong here.

mod fnv;
mod memory;
pub use fnv::Fnv64;
pub use memory::{Mapping, MemoryKind, Permissions};

/// An address in the address space's declared address unit (initially 8 bits).
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Addr(pub u64);

/// Absolute virtual time in picoseconds. Overflow must be handled, never wrapped.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct VTime(pub u64);

/// Non-negative virtual duration in picoseconds. Kept distinct from `VTime` so
/// an absolute timestamp cannot accidentally be passed where a delta is expected.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct VDelta(pub u64);

impl VTime {
    pub const ZERO: Self = Self(0);

    pub fn checked_add(self, delta: VDelta) -> Option<Self> {
        self.0.checked_add(delta.0).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeId(pub u32);

/// An ISA-defined register identifier, never a vendor backend enum value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegId(pub u32);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EventId(pub u64);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Security {
    Secure,
    NonSecure,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AccessKind {
    Data,
    Instruction,
    Debug,
    Dma,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Endian {
    Little,
    Big,
}

/// Attributes must survive every bus bridge, including DMA and debug accesses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Attr {
    pub master: NodeId,
    pub security: Security,
    pub privileged: bool,
    pub kind: AccessKind,
    pub exclusive: bool,
    pub byte_enable: u8,
    pub endian: Endian,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BusFault {
    Unmapped,
    Misaligned,
    InvalidSize,
    PermissionDenied,
    Reentrant,
    Device,
}

/// Valid MMIO transfer widths in bytes; invalid guest input is an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessSize {
    Byte,
    HalfWord,
    Word,
    DoubleWord,
}
impl AccessSize {
    pub const fn bytes(self) -> u8 {
        match self {
            Self::Byte => 1,
            Self::HalfWord => 2,
            Self::Word => 4,
            Self::DoubleWord => 8,
        }
    }
}
impl TryFrom<u8> for AccessSize {
    type Error = BusFault;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Byte),
            2 => Ok(Self::HalfWord),
            4 => Ok(Self::Word),
            8 => Ok(Self::DoubleWord),
            _ => Err(BusFault::InvalidSize),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SnapshotError {
    IdentityMismatch,
    VersionMismatch,
    GraphMismatch,
    InvalidState,
    /// A model, engine or host port has not implemented checkpoint support.
    CapabilityMissing,
}

/// Typed, in-memory snapshot contract. This is not a wire encoding; host
/// handles never belong in State. `validate` is side-effect free and lets an
/// aggregate validate every participant before any participant is restored.
pub trait Snapshot: Send {
    type State: Clone + Send;
    const SCHEMA_VERSION: u32;
    fn save(&self) -> Self::State;
    fn validate(&self, version: u32, state: &Self::State) -> Result<(), SnapshotError>;
    /// Validate before mutating; reject unsupported versions without changing self.
    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError>;
    /// Deterministic FNV-1a 64-bit digest of `state` at `version`.
    ///
    /// Not cryptographic, and not stable as a file format. Host handles, trait
    /// objects and raw pointers must not affect the result. The same version
    /// and state must always produce the same value in-process.
    fn fingerprint(version: u32, state: &Self::State) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_time_never_wraps() {
        assert_eq!(VTime(u64::MAX).checked_add(VDelta(1)), None);
        assert_eq!(VTime(3).checked_add(VDelta(4)), Some(VTime(7)));
    }

    #[test]
    fn rejects_invalid_guest_widths() {
        for width in 0..=255 {
            assert_eq!(
                AccessSize::try_from(width).is_ok(),
                [1, 2, 4, 8].contains(&width)
            );
        }
    }
}
