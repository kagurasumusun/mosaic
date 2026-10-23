#![forbid(unsafe_code)]

#[cfg(feature = "unicorn")]
mod unicorn;

fn main() -> std::process::ExitCode {
    match std::env::args().nth(1).as_deref() {
        None | Some("help" | "--help" | "-h") => {
            println!("Mosaic VM development CLI (no complete machine yet)\n\nUnicorn FlatView/MMIO binding smoke test:\n  cargo run -p mosaic-apps-cli --features unicorn -- unicorn-smoke");
            std::process::ExitCode::SUCCESS
        }
        Some("unicorn-smoke") => {
            #[cfg(feature = "unicorn")]
            match unicorn::run() {
                Ok(trace) => {
                    print!("{trace}");
                    std::process::ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("mosaic: {error}");
                    std::process::ExitCode::FAILURE
                }
            }
            #[cfg(not(feature = "unicorn"))]
            {
                eprintln!("mosaic: rebuild with --features unicorn");
                std::process::ExitCode::FAILURE
            }
        }
        Some(command) => {
            eprintln!("mosaic: unknown command {command}");
            std::process::ExitCode::FAILURE
        }
    }
}
