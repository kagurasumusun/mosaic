#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

//! Booting a real Linux kernel through the i386 boot protocol.
//!
//! The images live outside the repository (they are tens of megabytes), so a
//! missing image skips the test with a note instead of failing CI. Where the
//! image is present these tests drive the real kernel, not a fixture.

use mosaic_apps_vm_runner::linux_boot::{iso_file, BzImage};
use mosaic_apps_vm_runner::pc_boot::{run_linux_boot, PcBootOptions};

const TINYCORE: &str = "/tmp/images/tinycore-15.iso";

fn image(path: &str) -> Option<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            eprintln!("skip: {path} is not available ({error})");
            None
        }
    }
}

#[test]
fn iso_lookup_finds_kernel_and_initramfs() {
    let Some(iso) = image(TINYCORE) else { return };
    let kernel = iso_file(&iso, "boot/vmlinuz").expect("vmlinuz");
    let initrd = iso_file(&iso, "boot/core.gz").expect("core.gz");
    assert!(kernel.size > 1_000_000, "kernel size {}", kernel.size);
    assert!(initrd.size > 1_000_000, "initramfs size {}", initrd.size);
    // A directory is not a file: the lookup answers None, not the directory.
    assert!(iso_file(&iso, "boot").is_none());

    let parsed = BzImage::parse(&kernel.read(&iso).expect("kernel bytes")).expect("bzImage");
    println!(
        "protocol {:#06x} setup_sects {} loaded_high {} payload {} version {:?}",
        parsed.protocol,
        parsed.setup_sects,
        parsed.loaded_high(),
        parsed.payload.len(),
        parsed.version
    );
    assert!(parsed.loaded_high(), "a bzImage loads above 1 MiB");
    assert_eq!(parsed.payload_base(), 0x10_0000);
    assert_eq!(parsed.entry_segment(), 0x9020);
    assert!(parsed.version.contains("tinycore"));
}

#[test]
fn boot_a_real_linux_kernel() {
    let Some(iso) = image(TINYCORE) else { return };
    let options = PcBootOptions {
        quantum: 1_000_000,
        instruction_limit: 5_000_000,
        trace_instructions: 60,
        dumps: vec![(0x90200, 0x180)],
        watch_writes: vec![(0x902D1, 56)],
        ..PcBootOptions::default()
    };
    let report = run_linux_boot(
        Some(&iso),
        None,
        "boot/vmlinuz",
        Some("boot/core.gz"),
        "console=ttyS0,tty0 quiet",
        None,
        &options,
    )
    .expect("boot");
    println!("kernel {:?}", report.kernel);
    println!(
        "stop {} instructions {} eip {:#x}",
        report.stop_reason, report.instructions, report.eip
    );
    println!("regs {:08x?}", report.registers);
    println!("serial {:?}", String::from_utf8_lossy(&report.serial));
    println!("screen {:?}", report.screen);
    println!("quantum starts {:x?}", report.trace);
    println!("interrupts {:?}", report.interrupts);
    // The setup code in memory is what the guest executes; printing it is how
    // a placement bug is told apart from a decode bug.
    for (address, bytes) in &report.dumps {
        for (index, chunk) in bytes.chunks(16).enumerate() {
            let text = chunk
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<Vec<_>>()
                .join(" ");
            println!("{:08x}  {text}", address + (index * 16) as u64);
        }
    }
    println!("watched {} stores", report.watched_writes.len());
    for write in report.watched_writes.iter().take(8) {
        println!("  write {write:?}");
    }
    for (address, size, cs) in &report.instruction_trace {
        println!("  {address:#08x} size {size} cs {cs:#06x}");
    }
    for entry in report.interrupt_log.iter().rev().take(12).rev() {
        println!("  int log {entry:?}");
    }
    // The kernel's own code must have run: the loader only places the image
    // and jumps. Thousands of instructions of setup code is the proof.
    assert!(
        report.instructions > 1_000,
        "the kernel's setup code did not run: {} instructions",
        report.instructions
    );
}

/// The kernel's printk ring buffer lives in its own `.data`, and every message
/// is stored there as plain text before any console exists. Scraping it is how
/// a boot that produces no console output at all is still readable — the same
/// trick a `dmesg` from a serial-less machine needs.
fn printk_log(report: &mosaic_apps_vm_runner::pc_boot::PcBootReport) -> Vec<String> {
    let mut lines = Vec::new();
    for (_, bytes) in &report.dumps {
        let mut run = Vec::new();
        for &byte in bytes {
            if (0x20..0x7f).contains(&byte) || byte == b'\n' {
                run.push(byte);
            } else {
                if run.len() >= 8 {
                    lines.push(String::from_utf8_lossy(&run).trim().to_string());
                }
                run.clear();
            }
        }
        if run.len() >= 8 {
            lines.push(String::from_utf8_lossy(&run).trim().to_string());
        }
    }
    lines
}

#[test]
fn the_kernel_decompresses_and_starts_its_own_init() {
    let Some(iso) = image(TINYCORE) else { return };
    let options = PcBootOptions {
        quantum: 1_000_000,
        // Enough for the decompressor (~276M instructions) plus a few million
        // of the kernel's own early init.
        instruction_limit: 400_000_000,
        // The kernel's data/bss, where `prb` (the printk ring buffer) lives.
        dumps: vec![(0xb5_0000, 0x2_0000)],
        ..PcBootOptions::default()
    };
    let report = run_linux_boot(
        Some(&iso),
        None,
        "boot/vmlinuz",
        Some("boot/core.gz"),
        "console=tty0",
        None,
        &options,
    )
    .expect("boot");

    let screen = report.screen.join("\n");
    println!("screen:\n{screen}");
    let log = printk_log(&report);
    for line in log.iter().filter(|line| !line.is_empty()) {
        println!("log: {line}");
    }

    // The decompressor's own progress, written straight into the VGA window.
    assert!(
        screen.contains("Decompressing Linux"),
        "the decompressor never ran: {screen:?}"
    );
    assert!(
        screen.contains("Booting the kernel"),
        "the kernel was never entered: {screen:?}"
    );
    // And the kernel's own view of the machine, from its log buffer.
    assert!(
        log.iter()
            .any(|line| line.contains("Linux version") && line.contains("tinycore")),
        "no Linux version banner in the kernel log"
    );
    assert!(
        log.iter().any(|line| line.contains("BIOS-e820")),
        "the kernel did not accept the firmware's memory map"
    );
    assert!(
        log.iter().any(|line| line.contains("RAMDISK")),
        "the kernel did not find the initramfs the loader placed"
    );
}
