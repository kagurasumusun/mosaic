#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]
//! End-to-end tests of the execution entry point itself: the `mosaic-system`
//! command line, driven the way an operator drives it — real image on the
//! command line, machine-readable report out.

use std::process::Command;

use mosaic_tests_boot::{image_path, load_catalog, workspace_root, ASSETS_DIR, CATALOG};

fn binary() -> std::path::PathBuf {
    workspace_root().join("target/debug/mosaic-system")
}

fn require_binary() -> Option<std::path::PathBuf> {
    let path = binary();
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "skip: {} is not built; build with `cargo build -p mosaic-apps-system --features unicorn`",
            path.display()
        );
        None
    }
}

fn committed_tinycore() -> std::path::PathBuf {
    let catalog = load_catalog().expect(CATALOG);
    let entry = catalog
        .iter()
        .find(|entry| entry.id == "linux/tinycore-16-core")
        .expect("the committed Tiny Core entry is part of the catalog");
    let path = image_path(entry);
    assert!(
        path.exists(),
        "committed fixture {} must exist",
        path.display()
    );
    assert!(
        path.starts_with(workspace_root().join(ASSETS_DIR)),
        "the tinycore entry must stay committed"
    );
    path
}

fn temp_file(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mosaic-front-end-{name}-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn machine_help_lists_the_registry() {
    let Some(binary) = require_binary() else {
        return;
    };
    let output = Command::new(binary)
        .arg("-M")
        .arg("help")
        .output()
        .expect("mosaic-system runs");
    assert!(output.status.success(), "-M help must exit 0");
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("pc"), "the machine list names pc: {text}");
}

#[test]
fn an_unknown_machine_is_a_usage_error() {
    let Some(binary) = require_binary() else {
        return;
    };
    let output = Command::new(binary)
        .arg("-M")
        .arg("decstation")
        .arg("-cdrom")
        .arg(committed_tinycore())
        .output()
        .expect("mosaic-system runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "an unknown machine is a usage error"
    );
}

#[test]
fn a_missing_boot_source_is_a_usage_error() {
    let Some(binary) = require_binary() else {
        return;
    };
    let output = Command::new(binary).output().expect("mosaic-system runs");
    assert_eq!(
        output.status.code(),
        Some(2),
        "no drives and no kernel is a usage error"
    );
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("no boot source"),
        "the error says what is missing: {text}"
    );
}

#[test]
fn boots_the_committed_cd_and_writes_a_report() {
    let Some(binary) = require_binary() else {
        return;
    };
    let report = temp_file("tinycore");
    let serial = temp_file("tinycore-serial");
    let output = Command::new(binary)
        .arg("-cdrom")
        .arg(committed_tinycore())
        .arg("--instruction-limit")
        .arg("6000000")
        .arg("--report")
        .arg(&report)
        .arg("-serial")
        .arg(&serial)
        .arg("-q")
        .output()
        .expect("mosaic-system runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the real CD must boot: {stderr}");
    let json = std::fs::read_to_string(&report).expect("the report file was written");
    assert!(json.contains("\"machine\":\"pc\""), "{json}");
    assert!(
        json.contains("\"instructions\":6"),
        "the budget ran: {json}"
    );
    assert!(
        json.contains("ISOLINUX"),
        "the guest's banner is in the report: {json}"
    );
    // The guest's own output channels are captured separately.
    assert!(
        std::fs::metadata(&serial).is_ok(),
        "the serial file was written"
    );
}

#[test]
fn the_iso_kernel_operand_boots_the_kernel_directly() {
    let Some(binary) = require_binary() else {
        return;
    };
    // `-kernel <iso>!<inner>` is the documented way to boot the kernel out of
    // a CD without extracting it by hand.
    let iso = committed_tinycore();
    let kernel_spec = format!("{}!boot/vmlinuz", iso.display());
    let output = Command::new(binary)
        .arg("-kernel")
        .arg(&kernel_spec)
        .arg("-initrd")
        .arg(format!("{}!boot/core.gz", iso.display()))
        .arg("-append")
        .arg("console=tty0")
        .arg("--instruction-limit")
        .arg("4000000")
        .arg("--report")
        .arg(temp_file("kernel"))
        .arg("-q")
        .output()
        .expect("mosaic-system runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the kernel must boot: {stderr}");
}
