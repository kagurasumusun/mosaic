#![forbid(unsafe_code)]

#[cfg(feature = "unicorn")]
use mosaic_apps_vic_thermal::{run_vic_thermal, GOLDEN_OUTPUT};

#[cfg(feature = "unicorn")]
fn main() -> std::process::ExitCode {
    // Benchmark measurement (design §13 P3 exit): wall time for one
    // deterministic firmware run through the VIC aggregation path.
    let started = std::time::Instant::now();
    let report = match run_vic_thermal() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("vic-thermal: {error}");
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
    let micros = elapsed.as_micros().max(1);
    let instructions_per_second = report.hooks as u128 * 1_000_000 / micros;
    println!("elapsed_us={micros} instr_per_s={instructions_per_second}");

    if report.output != GOLDEN_OUTPUT {
        eprintln!("golden mismatch: got {:?}", report.output);
        return std::process::ExitCode::FAILURE;
    }
    // The VIC output must have reached the CPU IRQ line (ADR 0009): the edge
    // list is the composition-level proof of the aggregation path.
    if report.irq_edges != [(0, true)] {
        eprintln!("irq edge mismatch: got {:?}", report.irq_edges);
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(not(feature = "unicorn"))]
fn main() -> std::process::ExitCode {
    eprintln!("mosaic-vic-thermal: rebuild with --features unicorn");
    std::process::ExitCode::FAILURE
}
