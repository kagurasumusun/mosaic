#![cfg(feature = "riscv")]
//! P3 exit criterion (design §22 T3): a heterogeneous configuration boots
//! deterministically. Two shapes of the same schedule must agree on every
//! observable, and both must match the golden transcript proven in the
//! Python prototype.

use std::num::NonZeroU64;

use mosaic_apps_multicore::{
    run_multicore, ALT_QUANTUM, ALT_ROUNDS, GOLDEN_CONSOLE, GOLDEN_EDGE_STRINGS, GOLDEN_MHU_STAT,
    GOLDEN_SHARED, QUANTUM, ROUNDS,
};
use mosaic_contract_cpu_engine::EngineArch;

#[test]
fn heterogeneous_configuration_reaches_the_golden_outcome() {
    let report = run_multicore(NonZeroU64::new(QUANTUM).unwrap(), ROUNDS).expect("run");
    assert_eq!(report.console, GOLDEN_CONSOLE);
    assert_eq!(report.shared, GOLDEN_SHARED);
    assert_eq!(report.mhu_stat, GOLDEN_MHU_STAT);
    let edges = report.edge_strings();
    for (index, expected) in GOLDEN_EDGE_STRINGS.iter().enumerate() {
        assert_eq!(edges[index], *expected, "core {index} edge history");
    }
    assert_eq!(report.rounds, ROUNDS);
    // No timers exist in the scenario; every event drain must stay empty.
    assert_eq!(report.events, 0);
}

#[test]
fn quantum_shape_does_not_change_observable_outcomes() {
    let primary = run_multicore(NonZeroU64::new(QUANTUM).unwrap(), ROUNDS).expect("primary");
    let alternate =
        run_multicore(NonZeroU64::new(ALT_QUANTUM).unwrap(), ALT_ROUNDS).expect("alternate");
    assert_eq!(primary.console, alternate.console);
    assert_eq!(primary.shared, alternate.shared);
    assert_eq!(primary.mhu_stat, alternate.mhu_stat);
    assert_eq!(primary.core_edges, alternate.core_edges);
    // Instruction totals differ by shape (spinning cores retire more or fewer
    // instructions per slice); they are reported, never asserted.
}

#[test]
fn cpu_declarations_match_the_engines_they_drive() {
    assert_eq!(mosaic_cpu_arm_arm926ej_s::CORE.arch, EngineArch::ArmA32Le);
    assert_eq!(mosaic_cpu_arm_arm926ej_s::CORE.model, None);
    assert_eq!(mosaic_cpu_sifive_e31::CORE.arch, EngineArch::RiscV32Le);
    assert_eq!(mosaic_cpu_sifive_e31::CORE.model, Some("sifive-e31"));
}
