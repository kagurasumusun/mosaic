#![forbid(unsafe_code)]
//! P3 heterogeneous multicore composition (design §12.3–§12.5, §22).
//! The riscv feature gates the scenario exactly like the unicorn feature
//! gates the other apps (ADR 0007): without a CPU backend there is nothing
//! to compose.

#[cfg(feature = "riscv")]
mod multicore;

#[cfg(feature = "riscv")]
pub use multicore::{
    run_multicore, MulticoreReport, ALT_QUANTUM, ALT_ROUNDS, EVENT_BUDGET, GOLDEN_CONSOLE,
    GOLDEN_EDGE_STRINGS, GOLDEN_MHU_STAT, GOLDEN_SHARED, QUANTUM, ROUNDS,
};
