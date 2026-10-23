#![forbid(unsafe_code)]
//! Deterministic runtime services. Host time and model references stay outside.
//!
//! The event queue is single-threaded. Events are delivered only by an explicit
//! caller-driven pop; scheduling an event or signal never invokes a model.

mod context;
mod events;

pub use context::ModelContext;
pub use events::{Event, EventKind, EventQueue, QueueState};
