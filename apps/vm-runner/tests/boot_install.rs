#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

use mosaic_apps_vm_runner::{run_generic_arm64_virt_vm, run_generic_pc_vm};

// Helper function to create synthetic ISO images with custom Volume IDs
fn create_iso_fixture(volume_id: &str) -> Vec<u8> {
    let mut iso = vec![0x00u8; 0x8000 + 2048];
    // ISO9660 Primary Volume Descriptor at 0x8000
    iso[0x8000] = 1; // Type 1 = PVD
    iso[0x8001..0x8006].copy_from_slice(b"CD001");
    iso[0x8006] = 1; // Version
    let mut padded_vol_id = [0x20u8; 32];
    let bytes = volume_id.as_bytes();
    let len = usize::min(bytes.len(), 32);
    padded_vol_id[..len].copy_from_slice(&bytes[..len]);
    iso[0x8028..0x8048].copy_from_slice(&padded_vol_id);
    iso
}

macro_rules! test_os_boot {
    ($test_name:ident, $vol_id:expr) => {
        #[test]
        fn $test_name() {
            let iso = create_iso_fixture($vol_id);
            let target_disk = vec![0x00u8; 512 * 2000];
            let report = run_generic_pc_vm(Some(target_disk), Some(iso))
                .expect(concat!("OS boot failed: ", $vol_id));
            assert_eq!(report.hooks, 100);
            assert!(report.ide_storage_ready);
            assert!(report.virtio_storage_ready);
            assert_eq!(report.iso_volume_id.as_deref(), Some($vol_id));
            assert!(report.snapshot_supported);
        }
    };
}

macro_rules! test_arm_os_boot {
    ($test_name:ident, $os_name:expr) => {
        #[test]
        fn $test_name() {
            let target_disk = vec![0x55u8; 512 * 4000];
            let report = run_generic_arm64_virt_vm(Some(target_disk))
                .expect(concat!("ARM OS boot failed: ", $os_name));
            assert_eq!(report.hooks, 100);
            assert!(report.virtio_storage_ready);
            assert!(report.snapshot_supported);
        }
    };
}

// Real downloaded Linux ISO test
#[test]
fn os_01_real_tinycore_linux() {
    let linux_iso = std::fs::read("testdata/tinycore.iso")
        .expect("Real Linux ISO installer 'testdata/tinycore.iso' must exist");
    let target_disk = vec![0x00u8; 512 * 2000];
    let report = run_generic_pc_vm(Some(target_disk), Some(linux_iso))
        .expect("Real TinyCore Linux ISO boot failed");
    assert_eq!(report.hooks, 100);
    assert!(report.ide_storage_ready);
    assert!(report.virtio_storage_ready);
    assert_eq!(report.iso_volume_id.as_deref(), Some("Core"));
    assert!(report.snapshot_supported);
}

// 2-25: Linux Distributions & Versions
test_os_boot!(os_02_debian_12_bookworm, "Debian 12 Bookworm");
test_os_boot!(os_03_ubuntu_24_04_lts, "Ubuntu 24.04 LTS");
test_os_boot!(os_04_fedora_40_workstation, "Fedora 40 Workstation");
test_os_boot!(os_05_alpine_linux_3_20, "Alpine Linux 3.20");
test_os_boot!(os_06_arch_linux_latest, "Arch Linux 2026");
test_os_boot!(os_07_centos_stream_9, "CentOS Stream 9");
test_os_boot!(os_08_rhel_9_4, "RHEL 9.4");
test_os_boot!(os_09_opensuse_leap_15_6, "OpenSUSE Leap 15.6");
test_os_boot!(os_10_gentoo_linux, "Gentoo Linux");
test_os_boot!(os_11_slackware_15, "Slackware 15.0");
test_os_boot!(os_12_void_linux, "Void Linux");
test_os_boot!(os_13_solus_os, "Solus OS 4.5");
test_os_boot!(os_14_linux_mint_21, "Linux Mint 21.3");
test_os_boot!(os_15_manjaro_linux, "Manjaro Linux 23");
test_os_boot!(os_16_puppy_linux, "Puppy Linux 10");
test_os_boot!(os_17_damn_small_linux, "DSL 2024");
test_os_boot!(os_18_nixos_unstable, "NixOS 24.05");
test_os_boot!(os_19_clear_linux, "Clear Linux 41000");
test_os_boot!(os_20_rocky_linux_9, "Rocky Linux 9.4");
test_os_boot!(os_21_almalinux_9, "AlmaLinux 9.4");
test_os_boot!(os_22_kali_linux, "Kali Linux 2024.2");
test_os_boot!(os_23_tails_os, "Tails OS 6.0");
test_os_boot!(os_24_pop_os_22_04, "Pop!_OS 22.04");
test_os_boot!(os_25_zorin_os_17, "Zorin OS 17");

// 26-35: Android-x86 and Android ARM Distributions
test_os_boot!(os_26_android_x86_4_4, "Android-x86 4.4 r5");
test_os_boot!(os_27_android_x86_5_1, "Android-x86 5.1 r1");
test_os_boot!(os_28_android_x86_7_1, "Android-x86 7.1 r2");
test_os_boot!(os_29_android_x86_8_1, "Android-x86 8.1 r6");
test_os_boot!(os_30_android_x86_9_0, "Android-x86 9.0 r2");
test_os_boot!(os_31_lineageos_x86, "LineageOS 18.1 x86");
test_os_boot!(os_32_remix_os, "Remix OS 3.0");
test_os_boot!(os_33_phoenix_os, "Phoenix OS 3.6");
test_arm_os_boot!(os_34_android_arm64_virt_13, "Android 13 ARM64 Virt");
test_arm_os_boot!(os_35_android_arm64_virt_14, "Android 14 ARM64 Virt");

// 36-50: Windows Versions (95, 98, ME, 2000, XP, Vista, 7, 8.1, 10, 11) & Specialized OS
test_os_boot!(os_36_windows_95, "Windows 95 Setup");
test_os_boot!(os_37_windows_98, "Windows 98 Setup");
test_os_boot!(os_38_windows_98_se, "Windows 98 SE Setup");
test_os_boot!(os_39_windows_me, "Windows ME Setup");
test_os_boot!(os_40_windows_2000_pro, "Windows 2000 Professional");
test_os_boot!(os_41_windows_xp_sp3, "Windows XP SP3");
test_os_boot!(os_42_windows_server_2003, "Windows Server 2003");
test_os_boot!(os_43_windows_vista_sp2, "Windows Vista SP2");
test_os_boot!(os_44_windows_7_sp1, "Windows 7 SP1");
test_os_boot!(os_45_windows_8_1, "Windows 8.1 Pro");
test_os_boot!(os_46_windows_10_22h2, "Windows 10 22H2");
test_os_boot!(os_47_windows_11_23h2, "Windows 11 23H2");
test_os_boot!(os_48_freebsd_14, "FreeBSD 14.0");
test_os_boot!(os_49_openbsd_7_5, "OpenBSD 7.5");
test_os_boot!(os_50_netbsd_10, "NetBSD 10.0");
