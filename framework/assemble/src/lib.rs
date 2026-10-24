#![forbid(unsafe_code)]
//! L5 composition: wire the symbolic FlatView and the runtime [`Machine`] onto
//! a concrete CPU engine without making runtime and engine depend on each other.
//!
//! The engine contract stays engine-neutral: it only knows the
//! [`mosaic_contract_cpu_engine::EngineBus`] callback port. This crate provides
//! the runtime-backed port and the FlatView-derived engine map. Models and the
//! runtime never see vendor types; the engine never sees runtime types.

mod bus;
mod clockbind;
mod irq;

pub use bus::MachineBus;
pub use clockbind::instruction_period;
pub use irq::{EngineIrqSink, IrqMailbox, IrqMailboxState};

#[cfg(feature = "unicorn")]
mod driver;
#[cfg(feature = "unicorn")]
mod multicore;
#[cfg(feature = "unicorn")]
mod unicorn_bind;

#[cfg(feature = "unicorn")]
pub use driver::{DriverError, MachineDriver, StepReport};
#[cfg(feature = "unicorn")]
pub use multicore::{MulticoreDriver, MulticoreError, RoundReport, DEFAULT_PS_PER_INSTRUCTION};
#[cfg(feature = "unicorn")]
pub use unicorn_bind::{
    bind_flat_view, bind_flat_view_paged, bind_flat_view_shared, BindError, EngineMap, SharedRegion,
};
