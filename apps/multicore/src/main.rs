#![forbid(unsafe_code)]

#[cfg(feature = "riscv")]
use std::num::NonZeroU64;

#[cfg(feature = "riscv")]
use mosaic_apps_multicore::{
    run_multicore, EVENT_BUDGET, GOLDEN_CONSOLE, GOLDEN_EDGE_STRINGS, GOLDEN_MHU_STAT,
    GOLDEN_SHARED, QUANTUM, ROUNDS,
};

#[cfg(feature = "riscv")]
fn main() -> std::process::ExitCode {
    // P3 exit measurement (design §22): a heterogeneous three-core AMP
    // configuration (2x ARM926EJ-S + 1x RV32I) boots deterministically and
    // completes both handshakes over shared RAM and the MHU doorbells.
    let quantum = match NonZeroU64::new(QUANTUM) {
        Some(quantum) => quantum,
        None => {
            eprintln!("multicore: invalid quantum");
            return std::process::ExitCode::FAILURE;
        }
    };
    let started = std::time::Instant::now();
    let report = match run_multicore(quantum, ROUNDS) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("multicore: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let elapsed = started.elapsed();

    let console = String::from_utf8_lossy(&report.console);
    println!("console={console:?}");
    println!(
        "shared={:02X} {:02X} {:02X} {:02X}",
        report.shared[0], report.shared[1], report.shared[2], report.shared[3]
    );
    println!(
        "mhu_stat={} {} {}",
        report.mhu_stat[0], report.mhu_stat[1], report.mhu_stat[2]
    );
    let edges = report.edge_strings();
    println!(
        "edges c0={} c1={} c2={}",
        edges[0], edges[1], edges[2]
    );
    println!(
        "hooks=[{}, {}, {}] events={} rounds={} quantum={} budget={EVENT_BUDGET}",
        report.hooks[0], report.hooks[1], report.hooks[2], report.events, report.rounds, QUANTUM
    );
    let micros = elapsed.as_micros().max(1);
    let total_hooks = u128::from(report.hooks.iter().sum::<u64>());
    println!(
        "elapsed_us={micros} instr_per_s={}",
        total_hooks * 1_000_000 / micros
    );

    let mut ok = true;
    if report.console != GOLDEN_CONSOLE {
        eprintln!("golden console mismatch: got {:?}", report.console);
        ok = false;
    }
    if report.shared != GOLDEN_SHARED {
        eprintln!("golden shared mismatch: got {:02X?}", report.shared);
        ok = false;
    }
    if report.mhu_stat != GOLDEN_MHU_STAT {
        eprintln!("golden mhu mismatch: got {:?}", report.mhu_stat);
        ok = false;
    }
    for (index, expected) in GOLDEN_EDGE_STRINGS.iter().enumerate() {
        if edges[index] != *expected {
            eprintln!(
                "golden edge mismatch on core {index}: got {:?}, expected {expected:?}",
                edges[index]
            );
            ok = false;
        }
    }
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

#[cfg(not(feature = "riscv"))]
fn main() -> std::process::ExitCode {
    eprintln!("mosaic-multicore: rebuild with --features riscv");
    std::process::ExitCode::FAILURE
}
