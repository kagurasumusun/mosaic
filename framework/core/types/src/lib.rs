#![no_std]
#![forbid(unsafe_code)]
//! Shared VM vocabulary. No runtime or host services belong here.

/// An address in the address space's declared address unit (initially 8 bits).
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Addr(pub u64);

/// Virtual time in picoseconds. Overflow must be handled, never wrapped.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct VTime(pub u64);

impl VTime {
    pub const ZERO: Self = Self(0);

    pub fn checked_add(self, duration: Self) -> Option<Self> {
        self.0.checked_add(duration.0).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeId(pub u32);
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
}

/// Wire encoding is intentionally deferred; host handles never belong in State.
pub trait Snapshot: Send {
    type State: Clone + Send;
    const SCHEMA_VERSION: u32;
    fn save(&self) -> Self::State;
    /// Validate before mutating; reject unsupported versions without changing self.
    fn load(&mut self, version: u32, state: &Self::State) -> Result<(), SnapshotError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_time_never_wraps() {
        assert_eq!(VTime(u64::MAX).checked_add(VTime(1)), None);
        assert_eq!(VTime(3).checked_add(VTime(4)), Some(VTime(7)));
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
