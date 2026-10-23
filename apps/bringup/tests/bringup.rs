#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

use mosaic_apps_bringup::{run_bringup, GOLDEN_OUTPUT};

#[test]
fn firmware_boots_and_emits_the_golden_uart_output() {
    let report = run_bringup().expect("bringup run");
    assert_eq!(report.output, GOLDEN_OUTPUT);
    // The polling-free firmware produces no timer or UART interrupt edges.
    assert!(report.irq_edges.is_empty());
    assert_eq!(report.hooks, 64, "quantum exhausts in the final spin loop");
}

#[test]
fn independent_runs_are_deterministic() {
    let first = run_bringup().expect("first run");
    let second = run_bringup().expect("second run");
    assert_eq!(first, second);
    assert_eq!(first.output, GOLDEN_OUTPUT);
}
