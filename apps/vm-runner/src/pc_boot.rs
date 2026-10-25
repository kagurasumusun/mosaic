//! Real-mode PC bring-up: a firmware interim, an El Torito loader and a
//! running machine.
//!
//! The design document puts firmware where a PC actually has it: nowhere in
//! the CPU engine. A PC's firmware interface *is* the software-interrupt table,
//! so this module answers the BIOS calls a real bootloader makes (`INT 10h`
//! video, `INT 12h`/`INT 15h` memory, `INT 13h` disk, `INT 16h` keyboard,
//! `INT 1Ah` clock) through [`mosaic_engine_unicorn::InterruptService`], and
//! reaches the PIC/PIT/RTC/UART through the engine's port port. Nothing here
//! emulates a CPU: the guest's own 16-bit code drives everything.
//!
//! Scope of this slice, stated so it is not mistaken for a BIOS ROM:
//!
//! * the boot device is an ISO9660 image with an El Torito no-emulation entry
//!   (the layout every Linux/Windows CD uses) or a raw disk image whose first
//!   sector is a boot sector;
//! * `INT 13h` serves CHS and LBA reads from the attached image only;
//! * there is no option ROM scan, no PCI enumeration at the firmware level and
//!   no VGA mode beyond 80x25 text. A guest that needs those stops, and the
//!   report says where.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mosaic_contract_cpu_engine::{CpuEngine, RunOutcome, StopReason};
use mosaic_contract_mmio::MmioTarget;
use mosaic_contract_model::{ContextError, Ctx};
use mosaic_core_types::{AccessSize, Attr, BusFault, EventId, RegId, SignalId, VDelta, VTime};
use mosaic_engine_unicorn::{InterruptService, PortBus, SoftwareInterruptCpu, UnicornX86};
use mosaic_ip_x86_pic_8259::Pic8259;
use mosaic_ip_x86_pit_8254::Pit8254;
use mosaic_ip_x86_rtc_mc146818::RtcMc146818;

/// Conventional memory below the EBDA, as a PC reports it through `INT 12h`.
pub const CONVENTIONAL_KIB: u64 = 640;
/// VGA text window (80x25, two bytes per cell).
pub const VGA_TEXT_BASE: u64 = 0x000B_8000;
pub const VGA_TEXT_SIZE: u64 = 0x8000;
/// BIOS ROM window: mapped so a guest that reads it does not fault.
pub const BIOS_ROM_BASE: u64 = 0x000F_0000;
pub const BIOS_ROM_SIZE: u64 = 0x0001_0000;
/// Size of the RAM the machine has above 1 MiB.
pub const EXTENDED_RAM: u64 = 15 * 1024 * 1024;
/// Top of conventional RAM, and of the address space the firmware describes.
pub const CONVENTIONAL_TOP: u64 = 0x0009_FC00;
/// The VGA aperture (`A0000h..BFFFFh`), including the text window.
pub const VGA_APERTURE_BASE: u64 = 0x000A_0000;
pub const VGA_APERTURE_SIZE: u64 = 0x0002_0000;
/// Option ROM space between the VGA aperture and the BIOS window.
pub const ROM_GAP_BASE: u64 = 0x000C_0000;
pub const ROM_GAP_SIZE: u64 = 0x0003_0000;

/// Where an El Torito no-emulation image is conventionally loaded.
pub const DEFAULT_LOAD_SEGMENT: u16 = 0x07C0;
/// Extended BIOS data area size reported through `INT 15h`.
pub const EXTENDED_KIB: u64 = 15 * 1024;
/// `F000h`-relative offset of the interrupt trampolines (see [`firmware_image`]).
pub const BIOS_STUB_OFFSET: u64 = 0x1000;
/// `F000h`-relative reset vector: `F000:E05B` is where every PC starts.
pub const BIOS_RESET_OFFSET: u64 = 0xE05B;
/// Low-memory trampoline the machine starts at.
///
/// Unicorn's `EIP` is a segment-relative offset, and a direct `CS` write does
/// not install a segment base it will then honour. A `JMP FAR` does, so the
/// machine starts at `0000h:0500h` where five bytes name the boot point; the
/// far jump installs `CS` the way firmware would, and every later guest
/// segment load behaves like hardware.
pub const BOOT_TRAMPOLINE: u64 = 0x0500;

const REG_EAX: RegId = RegId(0);
const REG_ECX: RegId = RegId(1);
const REG_EDX: RegId = RegId(2);
const REG_EBX: RegId = RegId(3);
const REG_ESI: RegId = RegId(6);
const REG_EDI: RegId = RegId(7);
const REG_EIP: RegId = RegId(8);
const REG_EFLAGS: RegId = RegId(9);
const REG_CS: RegId = RegId(10);
const REG_DS: RegId = RegId(11);
const REG_ES: RegId = RegId(12);

const FLAG_CF: u64 = 1 << 0;
const FLAG_ZF: u64 = 1 << 6;

/// One attached BIOS disk.
#[derive(Clone, Debug)]
pub struct DiskImage {
    /// BIOS drive number (`0x00` first floppy, `0x80` first fixed disk).
    pub drive: u8,
    /// Logical block size the firmware reports to the guest.
    pub block_size: u32,
    pub data: Vec<u8>,
}

impl DiskImage {
    pub fn cdrom(drive: u8, data: Vec<u8>) -> Self {
        // Every ATAPI CD-ROM BIOS reports 2048-byte sectors, and bootloaders
        // size their reads from `INT 13h AH=48h` rather than assuming 512.
        Self {
            drive,
            block_size: 2048,
            data,
        }
    }

    pub fn fixed(drive: u8, data: Vec<u8>) -> Self {
        Self {
            drive,
            block_size: 512,
            data,
        }
    }

    fn blocks(&self) -> u64 {
        let block = u64::from(self.block_size);
        (self.data.len() as u64) / block
    }
}

/// Where the machine starts executing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootPoint {
    pub cs: u16,
    pub ip: u16,
    pub drive: u8,
}

/// Machine limits and fixtures.
#[derive(Clone, Debug)]
pub struct PcBootOptions {
    /// Instruction budget per quantum.
    pub quantum: u64,
    /// Total instruction budget for the whole run.
    pub instruction_limit: u64,
    /// Drive number handed to the boot image in `DL`.
    pub boot_drive: u8,
    /// How many recent instruction addresses to record, for diagnosis. Zero
    /// records nothing; the hook is expensive, so it stays off by default.
    pub trace_instructions: usize,
}

impl Default for PcBootOptions {
    fn default() -> Self {
        Self {
            quantum: 50_000,
            instruction_limit: 200_000_000,
            // `0x9F` is what ATAPI-CD firmware conventionally passes; a guest
            // that disagrees probes the drive table instead.
            boot_drive: 0x9F,
            trace_instructions: 0,
        }
    }
}

/// One disk read the firmware served.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskRead {
    pub drive: u8,
    /// First block, in the drive's own block size.
    pub lba: u64,
    pub blocks: u16,
    /// Guest address the data landed at.
    pub buffer: u64,
}

/// What the run observed. Everything a failing boot needs to be diagnosed.
#[derive(Clone, Debug, Default)]
pub struct PcBootReport {
    /// 80x25 text screen, trailing blanks trimmed per line.
    pub screen: Vec<String>,
    /// Bytes written to COM1 through the port space.
    pub serial: Vec<u8>,
    /// BIOS calls answered, by vector.
    pub interrupts: BTreeMap<u8, u32>,
    /// Disk reads served, in request order.
    pub disk_reads: Vec<DiskRead>,
    pub instructions: u64,
    pub stop_reason: String,
    /// Final architectural state, for triage.
    pub eip: u64,
    pub cs: u64,
    pub boot_point: Option<BootPoint>,
    /// `(cs, eip)` at each quantum boundary, in order. A boot that stops early
    /// is diagnosed from this trace, not from a debugger.
    pub trace: Vec<(u64, u64)>,
    /// First software interrupt the firmware did not answer, if any.
    pub unhandled_interrupt: Option<u8>,
    /// `(vector, cs, ip)` for every firmware call the guest made, in order.
    pub interrupt_log: Vec<(u8, u64, u64)>,
    /// The drive number the machine booted from.
    pub boot_drive: u8,
    /// Recent instruction addresses, oldest first; empty unless
    /// [`PcBootOptions::trace_instructions`] asked for them.
    pub instruction_trace: Vec<u64>,
    /// Bytes at the last recorded instruction, so a fault can be disassembled
    /// without a debugger.
    pub last_instruction: Vec<u8>,
}

impl PcBootReport {
    /// Screen and serial output joined, for assertions that do not care which
    /// channel a message came out of.
    pub fn text(&self) -> String {
        let mut text = self.screen.join("\n");
        if !self.serial.is_empty() {
            text.push('\n');
            text.push_str(&String::from_utf8_lossy(&self.serial));
        }
        text
    }
}

/// Minimal model context: the firmware services never schedule and never drive
/// signals, so they need no runtime services.
struct FirmwareCtx;

impl Ctx for FirmwareCtx {
    fn now(&self) -> VTime {
        VTime::ZERO
    }

    fn schedule(&mut self, _at: VTime, _token: u64) -> Result<EventId, ContextError> {
        Err(ContextError::ResourceExhausted)
    }

    fn cancel(&mut self, _event: EventId) -> bool {
        false
    }

    fn set_signal(&mut self, _signal: SignalId, _high: bool) -> Result<(), ContextError> {
        Ok(())
    }
}

fn word_attr() -> Attr {
    Attr {
        master: mosaic_core_types::NodeId(0),
        security: mosaic_core_types::Security::NonSecure,
        privileged: true,
        kind: mosaic_core_types::AccessKind::Data,
        exclusive: false,
        byte_enable: u8::MAX,
        endian: mosaic_core_types::Endian::Little,
    }
}

/// Serial transmitter state (NS16550D register set, COM1).
///
/// Held by the port router instead of a catalog model because it is the first
/// user: it moves to `catalog/ip/x86/uart-16550` the moment a second machine
/// wants it (design §4 Rule of two).
#[derive(Debug, Default)]
pub struct Serial16550 {
    pub dll: u8,
    pub dlm: u8,
    pub ier: u8,
    pub lcr: u8,
    pub mcr: u8,
    pub scr: u8,
    pub fifo_control: u8,
    /// Bytes handed to the host, in order.
    pub out: Vec<u8>,
    /// Line status bits beyond the two always-ready ones.
    pub extra_status: u8,
}

impl Serial16550 {
    /// Transmitter holding register empty, transmitter empty: both true because
    /// transmit is instantaneous in this slice.
    const LSR_TX_READY: u8 = 0x60;

    pub fn read(&mut self, offset: u16) -> u32 {
        let dlab = self.lcr & 0x80 != 0;
        u32::from(match offset {
            0 if dlab => self.dll,
            0 => 0, // RBR: no input in this slice
            1 if dlab => self.dlm,
            1 => self.ier,
            2 => 0x01, // IIR: no pending interrupt
            3 => self.lcr,
            4 => self.mcr,
            5 => Self::LSR_TX_READY | self.extra_status,
            6 => 0xB0, // MSR: CTS|DSR|DCD asserted
            7 => self.scr,
            _ => 0,
        })
    }

    pub fn write(&mut self, offset: u16, value: u8) {
        let dlab = self.lcr & 0x80 != 0;
        match offset {
            0 if dlab => self.dll = value,
            0 => self.out.push(value),
            1 if dlab => self.dlm = value,
            1 => self.ier = value,
            2 => self.fifo_control = value,
            3 => self.lcr = value,
            4 => self.mcr = value,
            // FCR shares offset 2 with IIR; both are accepted.
            6 => {}
            7 => self.scr = value,
            _ => {}
        }
    }
}

/// PC port space: the only way a real guest reaches these devices.
struct PcPorts {
    pic: Pic8259,
    pit: Pit8254,
    rtc: RtcMc146818,
    serial: Arc<Mutex<Serial16550>>,
}

impl PortBus for PcPorts {
    fn read(&mut self, port: u16, _size: usize) -> Result<u32, BusFault> {
        let mut ctx = FirmwareCtx;
        let attr = word_attr();
        match port {
            0x20..=0x21 => {
                let offset = u64::from(port - 0x20);
                let value = self.pic.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
                Ok((value & 0xFF) as u32)
            }
            0xA0..=0xA1 => {
                let offset = u64::from(port - 0xA0) + 2;
                let value = self.pic.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
                Ok((value & 0xFF) as u32)
            }
            0x40..=0x43 => {
                let offset = u64::from(port - 0x40);
                let value = self.pit.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
                Ok((value & 0xFF) as u32)
            }
            0x70..=0x71 => {
                let offset = u64::from(port - 0x70);
                let value = self.rtc.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
                Ok((value & 0xFF) as u32)
            }
            0x3F8..=0x3FF => {
                let offset = port - 0x3F8;
                let mut serial = self.serial.lock().map_err(|_| BusFault::Device)?;
                Ok(serial.read(offset))
            }
            // Unclaimed ports read as all ones, as an idle ISA bus does. A
            // bootloader probing an absent device has to see "no device".
            _ => Ok(0xFF),
        }
    }

    fn write(&mut self, port: u16, _size: usize, value: u32) -> Result<(), BusFault> {
        let mut ctx = FirmwareCtx;
        let attr = word_attr();
        let byte = value as u8;
        match port {
            0x20..=0x21 => {
                let offset = u64::from(port - 0x20);
                self.pic
                    .write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0xA0..=0xA1 => {
                let offset = u64::from(port - 0xA0) + 2;
                self.pic
                    .write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0x40..=0x43 => {
                let offset = u64::from(port - 0x40);
                self.pit
                    .write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0x70..=0x71 => {
                let offset = u64::from(port - 0x70);
                self.rtc
                    .write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0x3F8..=0x3FF => {
                let offset = port - 0x3F8;
                let mut serial = self.serial.lock().map_err(|_| BusFault::Device)?;
                serial.write(offset, byte);
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

/// Firmware interim answering the software interrupts a real bootloader uses.
struct PcBios {
    disks: Vec<DiskImage>,
    cursor_row: usize,
    cursor_col: usize,
    interrupts: BTreeMap<u8, u32>,
    disk_reads: Vec<DiskRead>,
    boot_drive: u8,
    /// The El Torito boot image the machine booted from, if it booted a CD.
    eltorito: Option<ElToritoImage>,
    /// First vector the firmware could not answer.
    unhandled: Option<u8>,
    /// `(vector, cs, ip)` for every firmware call, in order.
    log: Vec<(u8, u64, u64)>,
}

impl PcBios {
    fn new(disks: Vec<DiskImage>, boot_drive: u8, eltorito: Option<ElToritoImage>) -> Self {
        Self {
            disks,
            cursor_row: 0,
            cursor_col: 0,
            interrupts: BTreeMap::new(),
            disk_reads: Vec::new(),
            boot_drive,
            eltorito,
            unhandled: None,
            log: Vec::new(),
        }
    }

    fn disk(&self, drive: u8) -> Option<&DiskImage> {
        self.disks.iter().find(|disk| disk.drive == drive)
    }

    /// Write one character to the VGA text window and advance the cursor, the
    /// same visible effect `INT 10h AH=0Eh` has on hardware.
    fn teletype(&mut self, cpu: &mut dyn SoftwareInterruptCpu, ch: u8) {
        match ch {
            0x0D => self.cursor_col = 0,
            0x0A => {
                self.cursor_row += 1;
                if self.cursor_row >= 25 {
                    self.scroll(cpu);
                    self.cursor_row = 24;
                }
            }
            0x08 => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                }
            }
            ch if ch >= 0x20 => {
                let cell = VGA_TEXT_BASE + ((self.cursor_row * 80 + self.cursor_col) * 2) as u64;
                let _ = cpu.mem_write(cell, &[ch, 0x07]);
                self.cursor_col += 1;
                if self.cursor_col >= 80 {
                    self.cursor_col = 0;
                    self.cursor_row += 1;
                    if self.cursor_row >= 25 {
                        self.scroll(cpu);
                        self.cursor_row = 24;
                    }
                }
            }
            _ => {}
        }
    }

    fn scroll(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        // Move rows 1..24 up by one row and blank the last row.
        let mut row = [0u8; 160];
        for line in 1..25u64 {
            let from = VGA_TEXT_BASE + line * 160;
            if cpu.mem_read(from, &mut row).is_err() {
                return;
            }
            let to = VGA_TEXT_BASE + (line - 1) * 160;
            let _ = cpu.mem_write(to, &row);
        }
        let blank = [0x20u8, 0x07].repeat(80);
        let _ = cpu.mem_write(VGA_TEXT_BASE + 24 * 160, &blank);
    }

    fn read_screen_from(&self, cpu: &dyn SoftwareInterruptCpu) -> Vec<String> {
        let mut lines = Vec::new();
        let mut row = [0u8; 160];
        for line in 0..25u64 {
            if cpu.mem_read(VGA_TEXT_BASE + line * 160, &mut row).is_err() {
                break;
            }
            let text: String = row
                .chunks(2)
                .map(|cell| {
                    if cell[0].is_ascii_graphic() || cell[0] == b' ' {
                        cell[0] as char
                    } else {
                        ' '
                    }
                })
                .collect();
            lines.push(text.trim_end().to_string());
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        lines
    }

    fn set_ax(cpu: &mut dyn SoftwareInterruptCpu, value: u16) {
        let _ = cpu.reg_write(REG_EAX, u64::from(value));
    }

    fn ax(cpu: &dyn SoftwareInterruptCpu) -> u16 {
        cpu.reg_read(REG_EAX).unwrap_or(0) as u16
    }

    fn read_word(cpu: &dyn SoftwareInterruptCpu, address: u64) -> u16 {
        let mut buf = [0u8; 2];
        if cpu.mem_read(address, &mut buf).is_err() {
            return 0;
        }
        u16::from_le_bytes(buf)
    }

    fn read_dword(cpu: &dyn SoftwareInterruptCpu, address: u64) -> u32 {
        let mut buf = [0u8; 4];
        if cpu.mem_read(address, &mut buf).is_err() {
            return 0;
        }
        u32::from_le_bytes(buf)
    }

    fn segment_base(segment: u16) -> u64 {
        u64::from(segment) * 16
    }

    fn set_flag(cpu: &mut dyn SoftwareInterruptCpu, mask: u64, high: bool) {
        let mut flags = cpu.reg_read(REG_EFLAGS).unwrap_or(2);
        if high {
            flags |= mask;
        } else {
            flags &= !mask;
        }
        let _ = cpu.reg_write(REG_EFLAGS, flags);
    }

    /// `INT 13h`: CHS and LBA disk services over the attached images.
    fn disk_service(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        let ax = Self::ax(cpu);
        let function = (ax >> 8) as u8;
        let drive = cpu.reg_read(REG_EDX).unwrap_or(0) as u8;
        match function {
            0x00 => {
                Self::set_ax(cpu, (ax & 0xFF00) | 0x00);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x02 => self.chs_read(cpu, drive),
            0x08 => {
                // Geometry. A bootloader divides by these numbers, so every
                // one of them must be non-zero: DL = drive count, DH = last
                // head index, CL = last sector, CH = last cylinder.
                if let Some(_disk) = self.disk(drive) {
                    let _ = cpu.reg_write(REG_ECX, 0xFF3Fu64);
                    Self::set_ax(cpu, (ax & 0xFF00) | 0x00);
                    let _ = cpu.reg_write(REG_EDX, 0x0100u64);
                } else {
                    Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
                    Self::set_flag(cpu, FLAG_CF, true);
                }
            }
            0x41 => {
                // Extensions present, and a simple (non-EDD) interface.
                let _ = cpu.reg_write(REG_EBX, 0xAA55);
                Self::set_ax(cpu, (ax & 0xFF00) | 0x30);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x42 => self.extended_read(cpu, drive),
            0x48 => self.parameters(cpu, drive),
            0x4B => self.eltorito_service(cpu, drive),
            _ => {
                Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
                Self::set_flag(cpu, FLAG_CF, true);
            }
        }
    }

    /// `INT 13h AH=4Bh`: the El Torito extensions a CD boot loader needs to
    /// learn where its own boot image came from.
    fn eltorito_service(&mut self, cpu: &mut dyn SoftwareInterruptCpu, drive: u8) {
        let Some(image) = self.eltorito.clone() else {
            // Not a CD: no extensions, and `AH` reports why.
            Self::set_ax(cpu, 0x01B1);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        };
        let previous_ax = Self::ax(cpu);
        let buffer = Self::segment_base(cpu.reg_read(REG_DS).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_ESI).unwrap_or(0) as u16);
        match (previous_ax & 0xFF) as u8 {
            // Get status: 5-byte packet, byte 1 is 0 for "no emulation".
            0x00 => {
                let packet = [0x05u8, 0x00, 0x00, 0x00, 0x00];
                let _ = cpu.mem_write(buffer, &packet);
                Self::set_ax(cpu, 0x4B00);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            // Read boot information: the 0x13-byte specification packet.
            0x01 => {
                let mut packet = vec![0u8; 0x13];
                packet[0] = 0x13;
                packet[1] = image.media;
                packet[2] = drive;
                packet[3] = 0; // controller
                packet[4..8].copy_from_slice(&image.load_lba.to_le_bytes());
                // Device specification: 0 for an ATAPI drive, which is what a
                // Linux CD is.
                packet[8..10].copy_from_slice(&0u16.to_le_bytes());
                packet[10..12].copy_from_slice(&0u16.to_le_bytes());
                packet[12..14].copy_from_slice(&image.load_segment.to_le_bytes());
                packet[14..16].copy_from_slice(&image.sector_count.to_le_bytes());
                packet[16..18].copy_from_slice(&0u16.to_le_bytes());
                let _ = cpu.mem_write(buffer, &packet);
                Self::set_ax(cpu, 0x4B01);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            _ => {
                Self::set_ax(cpu, 0x01B4);
                Self::set_flag(cpu, FLAG_CF, true);
            }
        }
    }

    fn chs_read(&mut self, cpu: &mut dyn SoftwareInterruptCpu, drive: u8) {
        let ax = Self::ax(cpu);
        let count = (ax & 0xFF) as u64;
        let cx = cpu.reg_read(REG_ECX).unwrap_or(0);
        let cylinder = ((cx >> 8) & 0xFF) | ((cx & 0xC0) << 2);
        let sector = (cx & 0x3F).max(1);
        let head = (cpu.reg_read(REG_EDX).unwrap_or(0) >> 8) & 0xFF;
        let buffer = Self::segment_base(cpu.reg_read(REG_ES).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_EBX).unwrap_or(0) as u16);
        let Some(disk) = self.disk(drive).cloned() else {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        };
        let sectors_per_track = 18u64;
        let heads = 2u64;
        let lba = ((cylinder * heads + head) * sectors_per_track) + sector - 1;
        let block = u64::from(disk.block_size);
        // CHS reads name 512-byte sectors; a CD's 2048-byte sector covers four.
        let unit = block / 512;
        let lba = lba / unit.max(1);
        let length = count * 512;
        let start = (lba * block) as usize;
        let end = start.saturating_add(length as usize);
        if end > disk.data.len() || length == 0 {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        if cpu.mem_write(buffer, &disk.data[start..end]).is_err() {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        self.disk_reads.push(DiskRead {
            drive,
            lba,
            blocks: count as u16,
            buffer,
        });
        let _ = cpu.reg_write(REG_EAX, u64::from(ax & 0xFF00));
        Self::set_flag(cpu, FLAG_CF, false);
    }

    fn extended_read(&mut self, cpu: &mut dyn SoftwareInterruptCpu, drive: u8) {
        let ax = Self::ax(cpu);
        let packet = Self::segment_base(cpu.reg_read(REG_DS).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_ESI).unwrap_or(0) as u32);
        let count = Self::read_word(cpu, packet + 2);
        let offset = Self::read_word(cpu, packet + 4);
        let segment = Self::read_word(cpu, packet + 6);
        let lba = u64::from(Self::read_dword(cpu, packet + 8))
            | (u64::from(Self::read_dword(cpu, packet + 12)) << 32);
        let buffer = Self::segment_base(segment) + u64::from(offset);
        let Some(disk) = self.disk(drive).cloned() else {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        };
        let block = u64::from(disk.block_size);
        let start = (lba * block) as usize;
        let length = u64::from(count) * block;
        let end = start.saturating_add(length as usize);
        if count == 0 || end > disk.data.len() {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        if cpu.mem_write(buffer, &disk.data[start..end]).is_err() {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        self.disk_reads.push(DiskRead {
            drive,
            lba,
            blocks: count,
            buffer,
        });
        let _ = cpu.reg_write(REG_EAX, u64::from(ax & 0xFF00));
        Self::set_flag(cpu, FLAG_CF, false);
    }

    /// `INT 13h AH=48h`: extended drive parameters at `DS:SI`.
    fn parameters(&mut self, cpu: &mut dyn SoftwareInterruptCpu, drive: u8) {
        let ax = Self::ax(cpu);
        let Some(disk) = self.disk(drive).cloned() else {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        };
        let packet = Self::segment_base(cpu.reg_read(REG_DS).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_ESI).unwrap_or(0) as u32);
        let requested = Self::read_word(cpu, packet);
        if requested < 0x1A {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        let size = requested.min(0x1E);
        let mut out = vec![0u8; usize::from(size)];
        out[0..2].copy_from_slice(&size.to_le_bytes());
        // Bit 0: DMA boundary errors are impossible here; bit 1: CHS valid.
        out[2..4].copy_from_slice(&2u16.to_le_bytes());
        let cylinders = 1024u32;
        out[4..8].copy_from_slice(&cylinders.to_le_bytes());
        out[8..12].copy_from_slice(&16u32.to_le_bytes());
        out[12..16].copy_from_slice(&63u32.to_le_bytes());
        out[16..24].copy_from_slice(&disk.blocks().to_le_bytes());
        if size >= 0x1E {
            out[24..26].copy_from_slice(&(disk.block_size as u16).to_le_bytes());
        }
        if cpu.mem_write(packet, &out).is_err() {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        let _ = cpu.reg_write(REG_EAX, u64::from(ax & 0xFF00));
        Self::set_flag(cpu, FLAG_CF, false);
    }

    /// `INT 15h`: memory size and map, the calls every kernel probe makes.
    fn system_service(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        let ax = Self::ax(cpu);
        match ax {
            0x2400..=0x2403 => {
                // A20 gate: always enabled in this machine, nothing to do.
                Self::set_ax(cpu, 0x0001);
                let _ = cpu.reg_write(REG_EBX, 0);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x8800 => {
                Self::set_ax(cpu, EXTENDED_KIB as u16);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0xE801 => {
                // 1 MiB..16 MiB in KB, then the same in 64 KiB pages.
                let kib = (EXTENDED_KIB as u16).min(0x3C00);
                let _ = cpu.reg_write(REG_EAX, u64::from(kib));
                let _ = cpu.reg_write(REG_EBX, u64::from(kib / 64));
                let _ = cpu.reg_write(REG_ECX, 0);
                let _ = cpu.reg_write(REG_EDX, 0);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0xE820 => self.memory_map(cpu),
            0xE881 => self.memory_map(cpu),
            // Everything else (mode 86h delays, 0xC0 config, ...) reports
            // unhandled instead of pretending to succeed.
            _ => Self::set_flag(cpu, FLAG_CF, true),
        }
    }

    fn memory_map(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        // Usable, then everything the address decode reserves before 1 MiB,
        // then extended RAM. The map is what the machine actually maps: a
        // kernel that trusts it and faults has found a real bug.
        let mut entries: Vec<(u64, u64, u32)> = vec![
            (0, CONVENTIONAL_TOP, 1),
            (CONVENTIONAL_TOP, 0x10_0000 - CONVENTIONAL_TOP, 2),
            (0x0010_0000, EXTENDED_RAM, 1),
        ];
        let mut ebx = cpu.reg_read(REG_EBX).unwrap_or(0) as u32;
        if ebx as usize >= entries.len() {
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        let buffer = Self::segment_base(cpu.reg_read(REG_ES).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_EDI).unwrap_or(0) as u32);
        let (base, length, kind) = entries[ebx as usize];
        let mut entry = [0u8; 24];
        entry[0..8].copy_from_slice(&base.to_le_bytes());
        entry[8..16].copy_from_slice(&length.to_le_bytes());
        entry[16..20].copy_from_slice(&kind.to_le_bytes());
        if cpu.mem_write(buffer, &entry).is_err() {
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        ebx += 1;
        let _ = cpu.reg_write(
            REG_EBX,
            u64::from(if ebx as usize >= entries.len() {
                0
            } else {
                ebx
            }),
        );
        let _ = cpu.reg_write(REG_EAX, 0x534D_4150);
        let _ = cpu.reg_write(REG_ECX, 24);
        entries.clear();
        Self::set_flag(cpu, FLAG_CF, false);
    }

    /// `INT 10h`: text video services.
    fn video_service(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        match (Self::ax(cpu) >> 8) as u8 {
            0x0E => {
                let ch = (Self::ax(cpu) & 0xFF) as u8;
                self.teletype(cpu, ch);
            }
            0x13 => {
                // Write string: AL = mode, CX = count, BL = attribute.
                let count = cpu.reg_read(REG_ECX).unwrap_or(0) as usize;
                let buffer = Self::segment_base(cpu.reg_read(REG_ES).unwrap_or(0) as u16)
                    + u64::from(cpu.reg_read(REG_EBX).unwrap_or(0) as u16);
                for index in 0..count {
                    let mut byte = [0u8; 1];
                    if cpu.mem_read(buffer + index as u64, &mut byte).is_err() {
                        break;
                    }
                    self.teletype(cpu, byte[0]);
                }
            }
            0x00 | 0x01 => {
                // Mode set: only 80x25 text exists, and mode 3 is that mode.
                self.cursor_row = 0;
                self.cursor_col = 0;
                let blank = vec![0u8; (25 * 160) as usize];
                let _ = cpu.mem_write(VGA_TEXT_BASE, &blank);
                let current = Self::ax(cpu);
                Self::set_ax(cpu, 0x0300 | (current & 0xFF));
            }
            0x02 => {
                self.cursor_row = (cpu.reg_read(REG_EDX).unwrap_or(0) >> 8) as usize % 25;
                self.cursor_col = (cpu.reg_read(REG_EDX).unwrap_or(0) & 0xFF) as usize % 80;
            }
            0x03 => {
                let position = ((self.cursor_row as u64) << 8) | self.cursor_col as u64;
                let _ = cpu.reg_write(REG_EDX, position);
                let _ = cpu.reg_write(REG_ECX, 0x0607);
            }
            0x08 => {
                let cell = VGA_TEXT_BASE + ((self.cursor_row * 80 + self.cursor_col) * 2) as u64;
                let mut buf = [0u8; 2];
                let value = if cpu.mem_read(cell, &mut buf).is_ok() {
                    u64::from(buf[0])
                } else {
                    0
                };
                Self::set_ax(cpu, value as u16);
            }
            0x0F => {
                let current = Self::ax(cpu);
                Self::set_ax(cpu, 0x0003 | (current & 0xFF00));
            }
            // VESA and VGA extension calls: report "not supported" honestly.
            0x12 | 0x1A | 0x4F => {
                if (Self::ax(cpu) >> 8) as u8 == 0x4F {
                    let _ = cpu.reg_write(REG_EAX, 0x014F);
                } else {
                    Self::set_ax(cpu, 0);
                }
            }
            _ => {}
        }
    }

    fn clock_service(&mut self, cpu: &mut dyn SoftwareInterruptCpu) {
        // INT 1Ah: a fixed epoch is enough for a boot loader's banner.
        match (Self::ax(cpu) >> 8) as u8 {
            0x00 => {
                let _ = cpu.reg_write(REG_ECX, 0x0000_0000);
                let _ = cpu.reg_write(REG_EDX, 0x0000_0001);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x02 => {
                let _ = cpu.reg_write(REG_ECX, 0x0000_0000);
                let _ = cpu.reg_write(REG_EDX, 0x0000_0001);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x04 => {
                let _ = cpu.reg_write(REG_ECX, 0x0000_0000);
                let _ = cpu.reg_write(REG_EDX, 0x0000_0001);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            _ => Self::set_flag(cpu, FLAG_CF, true),
        }
    }
}

impl InterruptService for PcBios {
    fn service(&mut self, vector: u8, cpu: &mut dyn SoftwareInterruptCpu) -> bool {
        *self.interrupts.entry(vector).or_insert(0) += 1;
        self.log.push((
            vector,
            cpu.reg_read(REG_CS).unwrap_or(0),
            cpu.reg_read(REG_EIP).unwrap_or(0),
        ));
        match vector {
            0x10 => self.video_service(cpu),
            0x11 => {
                // Equipment word: one 80x25 colour display, one floppy.
                Self::set_ax(cpu, 0x0021);
            }
            0x12 => Self::set_ax(cpu, CONVENTIONAL_KIB as u16),
            0x13 => self.disk_service(cpu),
            0x15 => self.system_service(cpu),
            0x16 => {
                // No key is ever pending: report an empty buffer.
                Self::set_flag(cpu, FLAG_ZF, true);
                Self::set_ax(cpu, 0);
            }
            0x17 => Self::set_ax(cpu, 0x0000),
            0x1A => self.clock_service(cpu),
            // Timer tick, IRQ proxies and anything else: acknowledged, with
            // no hardware behind them yet.
            0x08 | 0x1B | 0x1C | 0x4A | 0x70 | 0x71 | 0x75 => {}
            // INT 19h is the firmware's own bootstrap; the runner performs it
            // before the first quantum, so a guest reaching it means the boot
            // image gave up.
            0x19 => {
                return false;
            }
            // A vector nobody claims — including every CPU exception, which
            // the interrupt hook also reports — stops the quantum so the run
            // reports the vector instead of spinning silently.
            _ => {
                self.unhandled = Some(vector);
                return false;
            }
        }
        true
    }
}

/// An El Torito no-emulation boot image extracted from an ISO9660 image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElToritoImage {
    pub load_segment: u16,
    pub sector_count: u16,
    pub load_lba: u32,
    pub media: u8,
    pub bytes: Vec<u8>,
}

const ISO_SECTOR: usize = 2048;

/// Locate and extract the El Torito boot image of an ISO9660 image.
///
/// Clean-room from the El Torito 1.0 specification: the Boot Record Volume
/// Descriptor is the volume descriptor with type 0, and its bytes 0x47..0x4B
/// name the boot catalog's LBA. The catalog's initial entry describes the
/// default boot image.
/// Bytes a real firmware would have left in low memory before the boot
/// image runs: the interrupt vector table, the BIOS data area and the
/// trampolines the vectors point at.
///
/// The vectors matter more than they look. A boot loader hooks a vector and
/// then *calls through* the copy it saved, so every `INT` entry must name
/// code that answers the call. Each trampoline is `INT n; IRET`: the `INT`
/// re-enters the [`InterruptService`], which answers it, and the `IRET`
/// returns to whoever chained. No vector is left pointing at zero, which is
/// what turns a chained call into a jump into empty memory.
pub fn firmware_image() -> FirmwareImage {
    let mut ivt = vec![0u8; 0x400];
    let mut stubs = Vec::new();
    for vector in 0u16..256 {
        // F000:(0x0100 + n * 4) -> INT n ; IRET
        let offset = BIOS_STUB_OFFSET as u16 + vector * 4;
        let at = usize::from(vector) * 4;
        ivt[at..at + 2].copy_from_slice(&offset.to_le_bytes());
        ivt[at + 2..at + 4].copy_from_slice(&0xF000u16.to_le_bytes());
        stubs.extend_from_slice(&[0xCD, vector as u8, 0xCF, 0x90]);
    }
    let mut bda = vec![0u8; 0x100];
    let put = |bda: &mut Vec<u8>, at: usize, value: u16| {
        bda[at - 0x400..at - 0x400 + 2].copy_from_slice(&value.to_le_bytes());
    };
    // Equipment word: one 80x25 colour display, one 1.44 MiB floppy.
    put(&mut bda, 0x410, 0x0021);
    // Conventional memory in KiB, which is what `INT 12h` also reports.
    put(&mut bda, 0x413, CONVENTIONAL_KIB as u16);
    // Keyboard buffer: empty, spanning 0x1E..0x3E in the data segment.
    put(&mut bda, 0x41A, 0x001E);
    put(&mut bda, 0x41C, 0x001E);
    put(&mut bda, 0x480, 0x001E);
    put(&mut bda, 0x482, 0x003E);
    // Current video state: mode 3, 80 columns, 4 KiB page at B800:0000.
    put(&mut bda, 0x449, 3);
    put(&mut bda, 0x44A, 80);
    put(&mut bda, 0x44C, 0x1000);
    put(&mut bda, 0x44E, 0);
    put(&mut bda, 0x450, 0);
    put(&mut bda, 0x460, 0x0607);
    put(&mut bda, 0x463, 0x3D4);
    put(&mut bda, 0x465, 0x0029);
    put(&mut bda, 0x466, 0x0030);
    // One fixed disk, and a warm-boot flag that is set.
    put(&mut bda, 0x475, 1);
    put(&mut bda, 0x472, 0x1234);
    // Timer tick count and rollover start at zero, as they do after a reset.
    let mut image = vec![0u8; 0x10000];
    image[..0x400].copy_from_slice(&ivt);
    image[0x400..0x500].copy_from_slice(&bda);
    let stub_at = BIOS_STUB_OFFSET as usize;
    image[stub_at..stub_at + stubs.len()].copy_from_slice(&stubs);
    FirmwareImage { image }
}

/// The firmware ROM image, ready to be written at [`BIOS_ROM_BASE`].
#[derive(Clone, Debug)]
pub struct FirmwareImage {
    image: Vec<u8>,
}

impl FirmwareImage {
    /// The low-memory half (`0000h..0500h`) the ROM's reset code copies into
    /// RAM: the real-mode interrupt table and the BIOS data area.
    pub fn low_memory(&self) -> &[u8] {
        &self.image[..0x500]
    }

    /// The whole ROM window, `F000h:0000h` onwards.
    pub fn rom(&self) -> &[u8] {
        &self.image
    }
}

pub fn el_torito_image(iso: &[u8]) -> Result<ElToritoImage, String> {
    // Volume descriptors start at LBA 16 and run until a terminator.
    let mut boot_record = None;
    for lba in 16..32usize {
        let at = lba * ISO_SECTOR;
        if at + ISO_SECTOR > iso.len() {
            break;
        }
        let descriptor = &iso[at..at + ISO_SECTOR];
        if &descriptor[1..6] != b"CD001" {
            return Err(format!("volume descriptor {lba} is not CD001"));
        }
        match descriptor[0] {
            0 => {
                if &descriptor[7..30] == b"EL TORITO SPECIFICATION" {
                    boot_record = Some(descriptor);
                }
            }
            255 => break,
            _ => {}
        }
    }
    let Some(boot_record) = boot_record else {
        return Err("no El Torito boot record volume descriptor".into());
    };
    let catalog_lba = u32::from_le_bytes([
        boot_record[0x47],
        boot_record[0x48],
        boot_record[0x49],
        boot_record[0x4A],
    ]);
    let catalog_at = catalog_lba as usize * ISO_SECTOR;
    if catalog_at + 64 > iso.len() {
        return Err("boot catalog lies outside the image".into());
    }
    let catalog = &iso[catalog_at..catalog_at + 64];
    // Validation entry: header id 0x01, a 16-bit word checksum over the first
    // 32 bytes that sums to zero, and the 0x55AA key. The 24-byte id string in
    // between is vendor text and is deliberately not interpreted.
    if catalog[0] != 0x01 {
        return Err("boot catalog validation entry has no header id".into());
    }
    let sum: u32 = catalog[..32]
        .chunks(2)
        .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
        .sum();
    if sum % 0x1_0000 != 0 {
        return Err("boot catalog validation entry checksum mismatch".into());
    }
    if catalog[30] != 0x55 || catalog[31] != 0xAA {
        return Err("boot catalog validation entry key is missing".into());
    }
    let entry = &catalog[32..64];
    if entry[0] != 0x88 {
        return Err("boot catalog initial entry is not bootable".into());
    }
    let media = entry[1] & 0x0F;
    let load_segment = u16::from_le_bytes([entry[2], entry[3]]);
    let sector_count = u16::from_le_bytes([entry[6], entry[7]]);
    let load_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
    let start = load_lba as usize * ISO_SECTOR;
    let length = usize::from(sector_count) * 512;
    if sector_count == 0 || start + length > iso.len() {
        return Err("boot image lies outside the image".into());
    }
    Ok(ElToritoImage {
        load_segment: if load_segment == 0 {
            DEFAULT_LOAD_SEGMENT
        } else {
            load_segment
        },
        sector_count,
        load_lba,
        media,
        bytes: iso[start..start + length].to_vec(),
    })
}

/// Boot a machine from an attached CD (El Torito) or disk image and run it.
pub fn run_pc_boot(
    iso: Option<Vec<u8>>,
    disk: Option<Vec<u8>>,
    options: &PcBootOptions,
) -> Result<PcBootReport, String> {
    let mut disks = Vec::new();
    if let Some(iso) = iso {
        let image = el_torito_image(&iso)?;
        let drive = options.boot_drive;
        disks.push(DiskImage::cdrom(drive, iso));
        let boot = BootPoint {
            cs: image.load_segment,
            ip: 0,
            drive,
        };
        return run_with(disks, Some(boot), options, Some(image));
    }
    if let Some(disk) = disk {
        let drive = 0x80;
        disks.push(DiskImage::fixed(drive, disk));
        let boot = BootPoint {
            cs: 0x07C0,
            ip: 0,
            drive,
        };
        return run_with(disks, Some(boot), options, None);
    }
    Err("a boot source (ISO or disk image) is required".into())
}

fn run_with(
    disks: Vec<DiskImage>,
    boot: Option<BootPoint>,
    options: &PcBootOptions,
    image: Option<ElToritoImage>,
) -> Result<PcBootReport, String> {
    let Some(boot) = boot else {
        return Err("no boot point".into());
    };
    let mut engine = UnicornX86::new().map_err(|error| format!("engine: {error:?}"))?;
    // Conventional RAM, the VGA text window and the BIOS window are all real
    // guest memory: the firmware writes the screen into the same window the
    // guest reads.
    engine
        .map_memory(
            mosaic_core_types::Addr(0),
            0xA_0000,
            mosaic_core_types::Permissions::RWX,
        )
        .map_err(|error| format!("ram map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_core_types::Addr(VGA_TEXT_BASE),
            VGA_TEXT_SIZE,
            mosaic_core_types::Permissions::RWX,
        )
        .map_err(|error| format!("vga map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_core_types::Addr(BIOS_ROM_BASE),
            BIOS_ROM_SIZE,
            mosaic_core_types::Permissions::RWX,
        )
        .map_err(|error| format!("rom map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_core_types::Addr(0x0010_0000),
            EXTENDED_RAM,
            mosaic_core_types::Permissions::RWX,
        )
        .map_err(|error| format!("extended ram map: {error:?}"))?;

    let serial = Arc::new(Mutex::new(Serial16550::default()));
    engine
        .attach_ports(Box::new(PcPorts {
            pic: Pic8259::new(),
            pit: Pit8254::new(),
            rtc: RtcMc146818::new(),
            serial: Arc::clone(&serial),
        }))
        .map_err(|error| format!("ports: {error:?}"))?;

    // A raw disk contributes its first sector to `7C00h`, which is where the
    // firmware's own bootstrap would have copied it.
    let boot_sector = if image.is_none() {
        let disk = disks.first().ok_or("boot disk missing")?;
        let size = (disk.block_size as usize).min(512);
        Some(
            disk.data
                .get(..size)
                .ok_or("boot disk has no first sector")?
                .to_vec(),
        )
    } else {
        None
    };

    // The firmware's reset code has already copied the vector table and the
    // BIOS data area into low memory by the time a boot image runs.
    let firmware = firmware_image();
    engine
        .write_memory(mosaic_core_types::Addr(0), firmware.low_memory())
        .map_err(|error| format!("firmware low memory: {error:?}"))?;
    engine
        .write_memory(mosaic_core_types::Addr(BIOS_ROM_BASE), firmware.rom())
        .map_err(|error| format!("firmware rom: {error:?}"))?;

    let bios = Arc::new(Mutex::new(PcBios::new(disks, boot.drive, image.clone())));
    engine
        .attach_interrupt_service(Box::new(SharedBios {
            inner: Arc::clone(&bios),
        }))
        .map_err(|error| format!("interrupt service: {error:?}"))?;

    // Load the boot image where the boot record says it lives.
    if let Some(image) = &image {
        let base = u64::from(image.load_segment) * 16;
        engine
            .write_memory(mosaic_core_types::Addr(base), &image.bytes)
            .map_err(|error| format!("boot image load: {error:?}"))?;
    }
    if let Some(sector) = &boot_sector {
        engine
            .write_memory(mosaic_core_types::Addr(0x7C00), sector)
            .map_err(|error| format!("boot sector load: {error:?}"))?;
    }

    // `JMP FAR seg:offset` to the boot point (see `BOOT_TRAMPOLINE`).
    let mut trampoline = vec![0xEA];
    trampoline.extend_from_slice(&boot.ip.to_le_bytes());
    trampoline.extend_from_slice(&boot.cs.to_le_bytes());
    engine
        .write_memory(mosaic_core_types::Addr(BOOT_TRAMPOLINE), &trampoline)
        .map_err(|error| format!("trampoline: {error:?}"))?;

    use mosaic_isa_x86::Register;
    engine
        .reg_write(Register::Cs.into(), 0)
        .map_err(|error| format!("cs: {error:?}"))?;
    engine
        .reg_write(Register::Eip.into(), BOOT_TRAMPOLINE)
        .map_err(|error| format!("eip: {error:?}"))?;
    engine
        .reg_write(Register::Ds.into(), 0)
        .map_err(|error| format!("ds: {error:?}"))?;
    engine
        .reg_write(Register::Es.into(), 0)
        .map_err(|error| format!("es: {error:?}"))?;
    engine
        .reg_write(Register::Ss.into(), 0)
        .map_err(|error| format!("ss: {error:?}"))?;
    engine
        .reg_write(Register::Esp.into(), 0x7C00)
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Edx.into(), u64::from(boot.drive))
        .map_err(|error| format!("dl: {error:?}"))?;
    engine
        .reg_write(Register::Eflags.into(), 2)
        .map_err(|error| format!("flags: {error:?}"))?;

    if options.trace_instructions > 0 {
        engine
            .trace_instructions(options.trace_instructions)
            .map_err(|error| format!("trace: {error:?}"))?;
    }

    let quantum = std::num::NonZeroU64::new(options.quantum.max(1)).ok_or("zero quantum")?;
    let mut trace = Vec::new();
    let mut executed = 0u64;
    let mut outcome = RunOutcome {
        reason: StopReason::BudgetExhausted,
        instruction_hooks: 0,
    };
    while executed < options.instruction_limit {
        trace.push((
            engine.reg_read(Register::Cs.into()).unwrap_or(0),
            engine.reg_read(Register::Eip.into()).unwrap_or(0),
        ));
        outcome = engine.run(quantum);
        executed += outcome.instruction_hooks;
        if outcome.reason != StopReason::BudgetExhausted {
            break;
        }
    }

    let last_traced = engine.instruction_trace().last().copied();
    let bios = bios.lock().map_err(|_| "bios lock".to_string())?;
    let screen = bios.read_screen_from(&FirmwareCpuView { engine: &engine });
    let mut serial_bytes = Vec::new();
    if let Ok(serial) = serial.lock() {
        serial_bytes = serial.out.clone();
    }
    Ok(PcBootReport {
        screen,
        serial: serial_bytes,
        interrupts: bios.interrupts.clone(),
        disk_reads: bios.disk_reads.clone(),
        instructions: executed,
        stop_reason: format!("{:?}", outcome.reason),
        eip: engine.reg_read(Register::Eip.into()).unwrap_or(0),
        cs: engine.reg_read(Register::Cs.into()).unwrap_or(0),
        boot_point: Some(boot),
        trace,
        unhandled_interrupt: bios.unhandled,
        interrupt_log: bios.log.clone(),
        boot_drive: bios.boot_drive,
        instruction_trace: engine.instruction_trace(),
        last_instruction: {
            let mut bytes = [0u8; 16];
            match last_traced {
                Some(address)
                    if engine
                        .read_memory(mosaic_core_types::Addr(address), &mut bytes)
                        .is_ok() =>
                {
                    bytes.to_vec()
                }
                _ => Vec::new(),
            }
        },
    })
}

/// The interrupt service runs inside an engine callback while the runner still
/// needs the BIOS state afterwards (`interrupts`, `disk_reads`), so the state
/// lives behind a mutex that the service borrows for one call.
struct SharedBios {
    inner: Arc<Mutex<PcBios>>,
}

impl InterruptService for SharedBios {
    fn service(&mut self, vector: u8, cpu: &mut dyn SoftwareInterruptCpu) -> bool {
        match self.inner.lock() {
            Ok(mut bios) => bios.service(vector, cpu),
            Err(_) => false,
        }
    }
}

/// Read-only CPU view used to snapshot the screen after the run.
struct FirmwareCpuView<'a> {
    engine: &'a UnicornX86,
}

impl SoftwareInterruptCpu for FirmwareCpuView<'_> {
    fn reg_read(&self, reg: RegId) -> Result<u64, mosaic_contract_cpu_engine::EngineError> {
        CpuEngine::reg_read(self.engine, reg)
    }

    fn reg_write(
        &mut self,
        _reg: RegId,
        _value: u64,
    ) -> Result<(), mosaic_contract_cpu_engine::EngineError> {
        Err(mosaic_contract_cpu_engine::EngineError::InvalidRegister)
    }

    fn mem_read(
        &self,
        address: u64,
        data: &mut [u8],
    ) -> Result<(), mosaic_contract_cpu_engine::EngineError> {
        CpuEngine::read_memory(self.engine, mosaic_core_types::Addr(address), data)
    }

    fn mem_write(
        &mut self,
        _address: u64,
        _data: &[u8],
    ) -> Result<(), mosaic_contract_cpu_engine::EngineError> {
        Err(mosaic_contract_cpu_engine::EngineError::Unmapped)
    }
}

/// Unused today, kept so `VDelta` stays imported for the delay services that
/// arrive with the PIT-backed clock.
const _: fn(VDelta) -> VTime = |delay| VTime::ZERO.checked_add(delay).unwrap_or(VTime::ZERO);
