#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

use mosaic_apps_sensorbus::{run_sensorbus, GOLDEN_OUTPUT};

#[test]
fn firmware_reaches_the_same_sensor_over_controller_and_bitbang_paths() {
    let report = run_sensorbus().expect("sensorbus run");
    assert_eq!(report.output, GOLDEN_OUTPUT);
    // P2 exit criterion: the bit-bang frame wrote ctrl_meas = 0x25 into the
    // very BME280 the DW-APB-I2C controller read; the mutation is observable
    // through the transaction path on the shared bus.
    assert_eq!(report.shared_ctrl_meas, 0x25);
    // The TCA9548A hides BME280-C until channel 3 is enabled, then routes it.
    assert!(report.mux_hidden_nack);
    assert_eq!(report.mux_routed_id, 0x60);
    // Polling firmware with all interrupt masks cleared: no line ever moves.
    assert!(report.irq_edges.is_empty());
    assert_eq!(report.hooks, 1024, "quantum exhausts in the final spin loop");
}

#[test]
fn independent_runs_are_deterministic() {
    let first = run_sensorbus().expect("first run");
    let second = run_sensorbus().expect("second run");
    assert_eq!(first, second);
    assert_eq!(first.output, GOLDEN_OUTPUT);
}
