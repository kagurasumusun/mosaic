#![cfg(feature = "unicorn")]
#![forbid(unsafe_code)]

//! Real bring-up checks: the guest's own code runs, so these tests are about
//! what a real boot loader achieved, not about synthetic register fixtures.

use mosaic_apps_vm_runner::pc_boot::{
    el_torito_image, firmware_image, run_pc_boot, PcBootOptions, BIOS_ROM_BASE, BIOS_STUB_OFFSET,
};

fn tinycore() -> Vec<u8> {
    std::fs::read("testdata/tinycore.iso").expect("testdata/tinycore.iso")
}

fn boot(
    iso: Option<Vec<u8>>,
    disk: Option<Vec<u8>>,
    limit: u64,
) -> mosaic_apps_vm_runner::PcBootReport {
    let options = PcBootOptions {
        quantum: 20_000,
        instruction_limit: limit,
        ..PcBootOptions::default()
    };
    run_pc_boot(iso, disk, &options).expect("boot")
}

fn boot_sector(code: &[u8]) -> Vec<u8> {
    let mut sector = vec![0u8; 512];
    sector[..code.len()].copy_from_slice(code);
    sector[510] = 0x55;
    sector[511] = 0xAA;
    sector
}

#[test]
fn tinycore_iso_exposes_an_el_torito_boot_image() {
    let iso = tinycore();
    let image = el_torito_image(&iso).expect("El Torito boot image");
    assert!(
        image.sector_count > 0,
        "boot image must describe its length"
    );
    assert_eq!(image.load_segment, 0x07C0);
    assert_eq!(image.bytes.len(), usize::from(image.sector_count) * 512);
    // The image is real code, not filler.
    assert!(image.bytes.iter().any(|byte| *byte != image.bytes[0]));
}

#[test]
fn firmware_image_publishes_vectors_and_a_data_area() {
    let firmware = firmware_image();
    let low = firmware.low_memory();
    // Every vector names a trampoline in the `F000h` window; none is zero.
    for vector in 0u16..256 {
        let at = usize::from(vector) * 4;
        let offset = u16::from_le_bytes([low[at], low[at + 1]]);
        let segment = u16::from_le_bytes([low[at + 2], low[at + 3]]);
        assert_eq!(segment, 0xF000, "vector {vector:#04x} must name the ROM");
        assert_eq!(offset, BIOS_STUB_OFFSET as u16 + vector * 4);
    }
    // BIOS data area: 640 KiB of conventional memory, mode 3, one fixed disk.
    assert_eq!(u16::from_le_bytes([low[0x413], low[0x414]]), 640);
    assert_eq!(low[0x449], 3, "current video mode is 80x25 text");
    assert_eq!(u16::from_le_bytes([low[0x44A], low[0x44B]]), 80);
    assert_eq!(u16::from_le_bytes([low[0x475], low[0x476]]), 1);
    // The trampoline for `INT 13h` is `INT 13h; IRET`.
    let stub = BIOS_STUB_OFFSET as usize + 0x13 * 4;
    assert_eq!(&firmware.rom()[stub..stub + 3], &[0xCD, 0x13, 0xCF]);
    assert_eq!(BIOS_ROM_BASE, 0x000F_0000);
}

#[test]
fn firmware_teletype_renders_text() {
    // mov ah,0x0E / mov al,'O' / int 0x10 / mov al,'K' / int 0x10 / jmp $
    let code = [
        0xB4, 0x0E, 0xB0, b'O', 0xCD, 0x10, 0xB0, b'K', 0xCD, 0x10, 0xEB, 0xFE,
    ];
    let report = boot(None, Some(boot_sector(&code)), 1_000);
    assert_eq!(report.interrupts.get(&0x10), Some(&2));
    assert_eq!(report.screen.first().map(String::as_str), Some("OK"));
}

#[test]
fn firmware_disk_read_serves_the_boot_disk() {
    // mov ax,0x0201 ; mov cx,0x0001 ; mov dx,0x0080 ; mov bx,0x0200 ;
    // int 0x13 ; jmp $
    let code = [
        0xB8, 0x01, 0x02, // mov ax,0x0201 (read 1 sector)
        0xB9, 0x01, 0x00, // mov cx,0x0001 (cylinder 0, sector 1)
        0xBA, 0x80, 0x00, // mov dx,0x0080 (drive 0, head 0)
        0xBB, 0x00, 0x02, // mov bx,0x0200 (buffer at 0000:0200)
        0xCD, 0x13, // int 0x13
        0xEB, 0xFE, // jmp $
    ];
    let report = boot(None, Some(boot_sector(&code)), 1_000);
    let read = report.disk_reads.first().copied().expect("one read");
    assert_eq!((read.drive, read.lba, read.blocks), (0x80, 0, 1));
    assert_eq!(read.buffer, 0x200, "the data must land at 0000:0200");
    assert_eq!(report.screen.len(), 0);
}

#[test]
fn tinycore_iso_boots_the_real_bootloader() {
    let report = boot(Some(tinycore()), None, 5_000_000);
    let text = report.screen.join("\n");
    println!("{text}");
    println!(
        "stop {} after {} instructions",
        report.stop_reason, report.instructions
    );
    assert!(report.instructions > 0, "the guest never executed anything");
    // The banner proves the guest's own 16-bit code ran, that its disk reads
    // were served and that its teletype writes reached the VGA window.
    assert!(
        text.contains("ISOLINUX"),
        "the boot loader never reached its banner: {text:?}"
    );
    assert!(
        report.interrupts.contains_key(&0x13),
        "the boot loader never read the CD: {:?}",
        report.interrupts
    );
    assert!(
        !report.disk_reads.is_empty(),
        "the boot loader never read the CD: {:?}",
        report.interrupts
    );
}

#[test]
fn diagnose_tinycore() {
    let options = PcBootOptions {
        quantum: 20_000,
        instruction_limit: 5_000_000,
        trace_instructions: 40,
        ..PcBootOptions::default()
    };
    let report = run_pc_boot(Some(tinycore()), None, &options).expect("boot");
    println!("stop {} eip {:#x}", report.stop_reason, report.eip);
    println!("screen {:?}", report.screen);
    let trace = &report.instruction_trace;
    for window in trace.windows(2) {
        let delta = window[1] as i64 - window[0] as i64;
        println!("{:#08x} {:+}", window[0], delta);
    }
    println!("last {:#x}", trace.last().copied().unwrap_or(0));
    println!("instruction {:02x?}", report.last_instruction);
    for read in &report.disk_reads {
        println!("read {read:?}");
    }
}
