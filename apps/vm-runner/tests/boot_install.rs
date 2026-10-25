#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

use mosaic_apps_vm_runner::{run_generic_arm64_virt_vm, run_generic_pc_vm};

#[test]
fn linux_boot_and_install_verification() {
    let linux_iso = std::fs::read("testdata/tinycore.iso")
        .expect("Real Linux ISO installer 'testdata/tinycore.iso' must exist");
    let target_disk = vec![0x00u8; 512 * 2000]; // Target disk

    let report = run_generic_pc_vm(Some(target_disk), Some(linux_iso))
        .expect("Linux ISO installer boot failed");

    assert_eq!(report.hooks, 100);
    assert!(report.ide_storage_ready);
    assert!(report.virtio_storage_ready);
    assert_eq!(report.iso_volume_id.as_deref(), Some("Core"));
}

#[test]
fn android_boot_and_install_verification() {
    let android_emmc = vec![0x55u8; 512 * 4000]; // Simulated Android eMMC image

    let report =
        run_generic_arm64_virt_vm(Some(android_emmc)).expect("Android virt image boot failed");

    assert_eq!(report.hooks, 100);
    assert!(report.virtio_storage_ready);
}

#[test]
fn windows_boot_and_install_verification() {
    let win_iso = vec![0xEBu8; 2048 * 1000]; // Simulated Windows ISO installer
    let target_disk = vec![0x00u8; 512 * 5000]; // Target disk

    let report = run_generic_pc_vm(Some(target_disk), Some(win_iso))
        .expect("Windows ISO installer boot failed");

    assert_eq!(report.hooks, 100);
    assert!(report.ide_storage_ready);
    assert!(report.virtio_storage_ready);
}

#[test]
fn boot_virtual_machine_without_media() {
    let report = run_generic_pc_vm(None, None).expect("Generic VM failed to start without media");
    assert_eq!(report.hooks, 100);
}
