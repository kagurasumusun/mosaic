#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]
//! The boot matrix: one generic harness, every real image in the catalog.
//!
//! There is deliberately no per-OS test code in this file. Each OS family is
//! one loop over its `tests/images.toml` entries, and `boot_case` does the
//! same three things for every image: resolve it (skip loudly when absent,
//! fail when it is not the bytes the catalog pinned), boot it on the named
//! machine, and assert the milestone the catalog recorded. Bringing a new
//! operating system into the suite means adding catalog rows — never a new
//! bespoke test — which is exactly the property QEMU's own functional tests
//! have and the old per-OS synthetic tests here lacked.

use mosaic_tests_boot::{boot_case, entries_for};

#[test]
fn linux_images() {
    for entry in entries_for("linux") {
        boot_case(&entry);
    }
}

#[test]
fn windows_images() {
    for entry in entries_for("windows") {
        boot_case(&entry);
    }
}

#[test]
fn android_images() {
    for entry in entries_for("android") {
        boot_case(&entry);
    }
}

#[test]
fn rtos_images() {
    for entry in entries_for("rtos") {
        boot_case(&entry);
    }
}

#[test]
fn other_images() {
    for entry in entries_for("other") {
        boot_case(&entry);
    }
}
