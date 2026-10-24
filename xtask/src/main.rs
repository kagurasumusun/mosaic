#![forbid(unsafe_code)]

use mosaic_xtask::{
    lint_catalog, lint_layers, lint_names, load_packages, workspace_root, CheckResult,
};

fn run() -> CheckResult<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    if matches!(command.as_str(), "help" | "--help" | "-h") {
        println!("cargo xtask <check | lint-layers | lint-names | lint-catalog | doctor>\n\ncheck runs layer, naming, and catalog validation. doctor reports the workspace status.");
        return Ok(());
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
