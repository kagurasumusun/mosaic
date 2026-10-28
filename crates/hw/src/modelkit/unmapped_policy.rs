//! The shared policy for offsets without a modeled register (ADR 0031 §6).
//!
//! One rule, every register model: reserved space either reads as zero and
//! ignores writes, or reports a guest-visible device fault. The policy is
//! a model parameter (PL011's `unmapped`, the SoC flashes' strict mode), so
//! the same device can be lenient on one board and strict on another.

/// Policy for offsets without a modeled register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnmappedPolicy {
    /// Read As Zero / Write Ignored, the default for reserved space.
    RazWi,
    /// Report a guest-visible device fault.
    Fault,
}
