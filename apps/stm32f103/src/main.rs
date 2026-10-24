#![forbid(unsafe_code)]

#[cfg(feature = "unicorn")]
use mosaic_apps_stm32f103::{run_stm32f103, Stm32F103Report, GOLDEN_OUTPUT};

#[cfg(feature = "unicorn")]
fn check(report: &Stm32F103Report) -> std::process::ExitCode {
    let console = String::from_utf8_lossy(&report.output);
    println!("console={console:?}");
    println!(
        "hooks={} events={} pin_edges={:?}",
        report.hooks, report.events, report.pin_edges
    );
    if report.output != GOLDEN_OUTPUT {
        eprintln!("golden mismatch: got {:?}", report.output);
        return std::process::ExitCode::FAILURE;
    }
    // The GPIO pin signal must have reached the harness sink (ADR 0008).
    // Output enable drives ODR low, then BSRR raises PA5.
    // Output enable drives the current ODR (low) before BSRR raises PA5.
    if report.pin_edges != [false, true] {
        eprintln!("pin edge mismatch: got {:?}", report.pin_edges);
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

#[cfg(feature = "unicorn")]
fn main() -> std::process::ExitCode {
    let first = match run_stm32f103() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("stm32f103: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let status = check(&first);
    if status == std::process::ExitCode::FAILURE {
        return status;
    }
    // Determinism: a second run must reproduce the first report exactly.
    match run_stm32f103() {
        Ok(second) if second == first => {
            println!("determinism: ok");
            status
        }
        Ok(second) => {
            eprintln!("determinism mismatch: {second:?} vs {first:?}");
            std::process::ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("stm32f103 (second run): {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(feature = "unicorn"))]
fn main() -> std::process::ExitCode {
    eprintln!("mosaic-stm32f103: rebuild with --features unicorn");
    std::process::ExitCode::FAILURE
}
