#![forbid(unsafe_code)]

use mosaic_xtask::{
    images, lint_catalog, lint_layers, lint_names, load_packages, workspace_root, CheckResult,
};

fn run() -> CheckResult<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        println!(
            "cargo xtask <check | lint-layers | lint-names | lint-catalog | images … | doctor>\n\
             \n\
             check runs layer, naming, and catalog validation. doctor reports the workspace status.\n\
             \n\
             images list                     show the boot-test image catalog and its local state\n\
             images fetch [--id ID]…         download `download` entries into test-images/ and verify digests\n\
             images verify                   verify the digests of images that are present"
        );
        return Ok(());
    }
    if command == "images" {
        let root = workspace_root()?;
        let mut arguments = std::env::args().skip(2).peekable();
        let subcommand = arguments.next().unwrap_or_else(|| "list".into());
        let mut only = Vec::new();
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--id" => {
                    if let Some(id) = arguments.next() {
                        only.push(id);
                    }
                }
                other => return Err(format!("unknown images argument {other}")),
            }
        }
        return match subcommand.as_str() {
            "list" => images::list(&root),
            "fetch" => images::fetch(&root, &only),
            "verify" => images::verify(&root),
            other => Err(format!("unknown images subcommand: {other}")),
        };
    }
    if !matches!(
        command.as_str(),
        "check" | "lint-layers" | "lint-names" | "lint-catalog" | "doctor"
    ) {
        return Err(format!("unknown command: {command}; run cargo xtask help"));
    }
    let root = workspace_root()?;
    let packages = load_packages(&root)?;
    let mut errors = Vec::new();
    if matches!(command.as_str(), "check" | "lint-layers" | "doctor") {
        errors.extend(lint_layers(&packages));
    }
    if matches!(command.as_str(), "check" | "lint-names" | "doctor") {
        errors.extend(lint_names(&root, &packages)?);
    }
    if matches!(command.as_str(), "check" | "lint-catalog" | "doctor") {
        errors.extend(lint_catalog(&root)?);
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    println!("{command}: OK ({} workspace crates)", packages.len());
    if command == "doctor" {
        println!(
            "Event/MMIO machine runtime with an opt-in Unicorn ARM MMIO binding; no complete machine."
        );
        println!("License/contribution policy and initial ISA/board require owner decisions.");
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
