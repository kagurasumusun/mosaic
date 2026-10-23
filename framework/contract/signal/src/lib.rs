#![no_std]
#![forbid(unsafe_code)]
//! Logical signal contract; net resolution and IRQ controllers are separate.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Trigger {
    LevelHigh,
    LevelLow,
    RisingEdge,
    FallingEdge,
    BothEdges,
}

pub trait SignalSink: Send {
    /// Called by the runtime after the originating callback has returned.
    fn set_level(&mut self, high: bool);
}
