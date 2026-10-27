//! Electrical net contract for digital pins.
//!
//! A net resolves the simultaneous drive states of every attached node into
//! one level, applying pull resistors, open-drain release tristate and
//! collision detection (構成設計書 §13.3). Resolution itself is implemented by
//! `mosaic-interconnect-gpio-net`; this crate only fixes the vocabulary.

use crate::gpio::PinLevel;
use crate::model::Ctx;

/// How one node drives the net at a given moment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NetDrive {
    /// Released; the node contributes no level.
    HighZ,
    /// Actively pulls the net low.
    StrongLow,
    /// Actively drives the net high.
    StrongHigh,
    /// Open-drain output: low while asserted, released otherwise.
    OpenDrainLow,
}

/// Pull resistor declared by a node when it attaches to the net.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum NetPull {
    None,
    Up,
    Down,
}

/// Handle returned when a node attaches; drives must quote it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetNode(pub u32);

/// Resolved net state. `collision` marks simultaneously conflicting strong
/// drivers; the level is still deterministic so simulations stay reproducible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct NetLevel {
    pub level: PinLevel,
    pub collision: bool,
}

impl NetLevel {
    pub const fn new(level: PinLevel, collision: bool) -> Self {
        Self { level, collision }
    }
}

/// A digital electrical net. Implementations live in the interconnect layer
/// and must resolve deterministically without host time or randomness.
pub trait DigitalNet: Send {
    /// Attach a node with its pull resistor and receive its handle.
    fn attach(&mut self, pull: NetPull) -> NetNode;

    /// Update one node's drive state and re-resolve the net level.
    fn drive(&mut self, ctx: &mut dyn Ctx, node: NetNode, drive: NetDrive);

    /// Current resolved level.
    fn level(&self) -> NetLevel;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_level_is_constructible_for_conformance_harnesses() {
        let level = NetLevel::new(PinLevel::High, true);
        assert_eq!(level.level, PinLevel::High);
        assert!(level.collision);
    }
}
