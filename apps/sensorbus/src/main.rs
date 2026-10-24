#![forbid(unsafe_code)]

#[cfg(feature = "unicorn")]
use mosaic_apps_sensorbus::{run_sensorbus, GOLDEN_OUTPUT};

#[cfg(feature = "unicorn")]
fn main() -> std::process::ExitCode {
    // P2 exit measurement (design §13): one deterministic firmware run that
    // reaches the same sensor part over the controller and the bit-bang path.
    let started = std::time::Instant::now();
    let report = match run_sensorbus() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("sensorbus: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let elapsed = started.elapsed();

    let console = String::from_utf8_lossy(&report.output);
    println!("console={console:?}");
    println!(
        "hooks={} events={} irq_edges={}",
        report.hooks,
        report.events,
        report.irq_edges.len()
    );
    println!(
        "probes ctrl_meas={:02X} mux_hidden={} mux_routed={:02X} phases bus={} pins={}",
        report.shared_ctrl_meas,
        report.mux_hidden_nack,
        report.mux_routed_id,
        report.bus_phases,
        report.pin_phases,
    );
    let micros = elapsed.as_micros().max(1);
    let instructions_per_second = report.hooks as u128 * 1_000_000 / micros;
    println!("elapsed_us={micros} instr_per_s={instructions_per_second}");

    if report.output != GOLDEN_OUTPUT {
        eprintln!("golden mismatch: got {:?}", report.output);
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(not(feature = "unicorn"))]
fn main() -> std::process::ExitCode {
    eprintln!("mosaic-sensorbus: rebuild with --features unicorn");
    std::process::ExitCode::FAILURE
}
