#![forbid(unsafe_code)]

#[cfg(feature = "unicorn")]
use mosaic_apps_vm_runner::run_generic_pc_vm;

fn main() -> std::process::ExitCode {
    #[cfg(feature = "unicorn")]
    {
        println!("Mosaic Generic VM Runner starting...");
        match run_generic_pc_vm(Some(vec![0u8; 1024 * 1024]), Some(vec![0u8; 2048 * 100])) {
            Ok(report) => {
                println!("Generic VM execution report: {report:?}");
                std::process::ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("vm-runner error: {error}");
                std::process::ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(feature = "unicorn"))]
    {
        eprintln!("mosaic-apps-vm-runner: rebuild with --features unicorn");
        std::process::ExitCode::FAILURE
    }
}
