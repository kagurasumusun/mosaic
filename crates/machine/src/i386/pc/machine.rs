//! The `pc` machine: a firmware interim, an El Torito loader, a BIOS drive
//! table and a running machine — `board:ibm/pc`, the machine a system
//! front-end runs under the name `-M pc`.
//!
//! This crate is the QEMU `hw/i386/pc_piix.c` + `hw/i386/pc.c` role: the
//! machine model itself, living in the catalog where every later machine
//! (`board:*/virt`, `board:*/goldfish`, ...) will live beside it. The
//! front-end (the `mosaic` crate, QEMU's `system/vl.c`) only names machines and
//! translates its options into [`PcMachineConfig`]; it owns no machine code.
//!
//! The design document puts firmware where a PC actually has it: nowhere in
//! the CPU engine. A PC's firmware interface *is* the software-interrupt table,
//! so this module answers the BIOS calls a real bootloader makes (`INT 10h`
//! video, `INT 12h`/`INT 15h` memory, `INT 13h` disk, `INT 16h` keyboard,
//! `INT 1Ah` clock) through [`mosaic_accel_unicorn::InterruptService`], and
//! reaches the PIC/PIT/RTC/UART through the engine's port port. Nothing here
//! emulates a CPU: the guest's own 16-bit code drives everything.
//!
//! Scope of this machine, stated so it is not mistaken for a BIOS ROM:
//!
//! * the boot device is an ISO9660 image with an El Torito no-emulation entry
//!   (the layout every Linux/Windows CD uses) or a raw disk/floppy image whose
//!   first sector is a boot sector;
//! * `INT 13h` serves CHS and LBA reads from the attached images only;
//! * there is no option ROM scan, no PCI enumeration at the firmware level and
//! * no VGA mode beyond 80x25 text. A guest that needs those stops, and the
//!   report says where.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use mosaic_accel_unicorn::{InterruptService, PortBus, SoftwareInterruptCpu, UnicornX86Boot};
use mosaic_contract::cpu_engine::{CpuEngine, RunOutcome, StopReason};
use mosaic_contract::mmio::MmioTarget;
use mosaic_contract::model::{ContextError, Ctx};
use mosaic_exec::{AccessSize, Attr, BusFault, EventId, RegId, SignalId, VDelta, VTime};

pub use super::DirectKernel;

use mosaic_boot::format::iso9660::{el_torito_image, ElToritoImage};
use mosaic_boot::proto::x86_linux::SETUP_BASE;
use mosaic_hw::intc::pic_8259::Pic8259;
use mosaic_hw::rtc::mc146818::RtcMc146818;
use mosaic_hw::timer::pit_8254::Pit8254;

/// Conventional memory below the EBDA, as a PC reports it through `INT 12h`.
pub const CONVENTIONAL_KIB: u64 = 640;
/// VGA text window (80x25, two bytes per cell).
pub const VGA_TEXT_BASE: u64 = 0x000B_8000;
pub const VGA_TEXT_SIZE: u64 = 0x8000;
/// BIOS ROM window: mapped so a guest that reads it does not fault.
pub const BIOS_ROM_BASE: u64 = 0x000F_0000;
pub const BIOS_ROM_SIZE: u64 = 0x0001_0000;
/// Size of the RAM the machine has above 1 MiB, as a default machine of
/// 64 MiB reports it. The configuration's `-m` overrides this everywhere.
pub const EXTENDED_RAM: u64 = 63 * 1024 * 1024;
/// Largest extended RAM the machine maps, in KiB: just under 4 GiB, keeping
/// the top of the 32-bit space for MMIO the way a PC does.
pub const MAX_EXTENDED_KIB: u64 = 0xFFFC00;
/// Top of conventional RAM, and of the address space the firmware describes.
pub const CONVENTIONAL_TOP: u64 = 0x0009_FC00;
/// The VGA aperture (`A0000h..BFFFFh`), including the text window.
pub const VGA_APERTURE_BASE: u64 = 0x000A_0000;
pub const VGA_APERTURE_SIZE: u64 = 0x0002_0000;
/// Option ROM space between the VGA aperture and the BIOS window.
pub const ROM_GAP_BASE: u64 = 0x000C_0000;
pub const ROM_GAP_SIZE: u64 = 0x0003_0000;
/// Local APIC register page. A kernel reads `APIC_ID` before it knows whether
/// an APIC is even present, so the page must decode; nothing here is a timer
/// yet, and the registers read back as a zeroed, idle LAPIC.
pub const LOCAL_APIC_BASE: u64 = 0xFEE0_0000;
pub const LOCAL_APIC_SIZE: u64 = 0x0000_1000;

/// Extended memory reported through `INT 15h`, in KiB, for the default
/// machine size. The default load segment of an El Torito image is a format
/// fact and lives in `mosaic-modelkit-iso9660` with its parser.
pub const EXTENDED_KIB: u64 = EXTENDED_RAM / 1024;
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
/// `CR0`: bit 0 is `PE`, which says whether selectors are indices or paragraphs.
const REG_CR0: RegId = RegId(16);
const REG_CS: RegId = RegId(10);
const REG_DS: RegId = RegId(11);
const REG_ES: RegId = RegId(12);

const FLAG_CF: u64 = 1 << 0;
const FLAG_ZF: u64 = 1 << 6;

/// The media a BIOS drive holds. The geometry a `CHS` read decodes with and
/// the drive number the machine assigns both depend on this.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriveMedia {
    Floppy,
    HardDisk,
    Cdrom,
}

/// One attached BIOS disk.
#[derive(Clone, Debug)]
pub struct DiskImage {
    /// BIOS drive number (`0x00` first floppy, `0x80` first fixed disk).
    pub drive: u8,
    /// Logical block size the firmware reports to the guest.
    pub block_size: u32,
    pub media: DriveMedia,
    pub data: Vec<u8>,
}

impl DiskImage {
    pub fn cdrom(drive: u8, data: Vec<u8>) -> Self {
        // Every ATAPI CD-ROM BIOS reports 2048-byte sectors, and bootloaders
        // size their reads from `INT 13h AH=48h` rather than assuming 512.
        Self {
            drive,
            block_size: 2048,
            media: DriveMedia::Cdrom,
            data,
        }
    }

    pub fn fixed(drive: u8, data: Vec<u8>) -> Self {
        Self {
            drive,
            block_size: 512,
            media: DriveMedia::HardDisk,
            data,
        }
    }

    pub fn floppy(drive: u8, data: Vec<u8>) -> Self {
        Self {
            drive,
            block_size: 512,
            media: DriveMedia::Floppy,
            data,
        }
    }

    fn blocks(&self) -> u64 {
        let block = u64::from(self.block_size);
        (self.data.len() as u64) / block
    }

    /// Geometry a `CHS` address on this media decodes with:
    /// `(heads, sectors per track)`. Floppies are 2 heads / 18 sectors (the
    /// 1.44 MiB format), hard disks 16 heads / 63 sectors — the numbers
    /// `INT 13h AH=08h` reports for them.
    fn geometry(&self) -> (u64, u64) {
        match self.media {
            DriveMedia::Floppy => (2, 18),
            _ => (16, 63),
        }
    }
}

/// How the PC picks what to execute first.
#[derive(Clone, Debug)]
enum PcBootPlan {
    /// An El Torito no-emulation boot image from an attached CD.
    ElTorito { image: ElToritoImage, drive: u8 },
    /// The first sector of a floppy or hard disk, at `0000:7C00h`.
    BootSector { drive: u8 },
    /// A `bzImage` placed by the documented i386 boot protocol.
    Linux(DirectKernel),
}

/// A PC as the front-end named it: RAM size, drive images in attachment
/// order, a boot order over media, and an optional direct kernel.
///
/// Drive numbers are assigned the way a PC BIOS assigns them: floppies from
/// `0x00`, hard disks from `0x80`, and the CD-ROM at `0x9F` — the number the
/// El Torito specification calls the conventional CD boot drive.
#[derive(Clone, Debug)]
pub struct PcMachineConfig {
    /// Total machine RAM in KiB, including the low megabyte. The machine
    /// splits it into conventional memory (fixed by the PC memory map) and
    /// extended memory above 1 MiB.
    pub ram_kib: u64,
    /// The microarchitecture the CPU impersonates (QEMU `-cpu`). The backend
    /// default predates BMI2, which every `x86-64-v2` kernel uses.
    pub cpu_model: mosaic_accel_unicorn::X86CpuModel,
    pub floppies: Vec<Vec<u8>>,
    pub hard_disks: Vec<Vec<u8>>,
    pub cdrom: Option<Vec<u8>>,
    /// QEMU `-boot` order: each character names a media — `'a'` first floppy,
    /// `'c'` first hard disk, `'d'` the CD-ROM. The first media with an image
    /// that has a boot record wins.
    pub boot_order: String,
    /// A direct kernel boot, which wins over any boot order.
    pub kernel: Option<DirectKernel>,
}

impl PcMachineConfig {
    pub fn new(ram_kib: u64) -> Self {
        Self {
            ram_kib,
            cpu_model: mosaic_accel_unicorn::X86CpuModel::Haswell,
            floppies: Vec::new(),
            hard_disks: Vec::new(),
            cdrom: None,
            boot_order: "dca".into(),
            kernel: None,
        }
    }

    /// QEMU `-cpu`: which microarchitecture the core impersonates.
    pub fn with_cpu_model(&mut self, model: mosaic_accel_unicorn::X86CpuModel) -> &mut Self {
        self.cpu_model = model;
        self
    }

    pub fn attach_floppy(&mut self, data: Vec<u8>) -> &mut Self {
        self.floppies.push(data);
        self
    }

    pub fn attach_hard_disk(&mut self, data: Vec<u8>) -> &mut Self {
        self.hard_disks.push(data);
        self
    }

    pub fn attach_cdrom(&mut self, data: Vec<u8>) -> Result<&mut Self, String> {
        // Refuse at configuration time, not mid-boot: an image without a boot
        // record is an operator error and the message should say so.
        el_torito_image(&data)?;
        self.cdrom = Some(data);
        Ok(self)
    }

    pub fn with_boot_order(&mut self, order: String) -> &mut Self {
        self.boot_order = order;
        self
    }

    pub fn with_kernel(&mut self, kernel: DirectKernel) -> &mut Self {
        self.kernel = Some(kernel);
        self
    }

    /// Assign BIOS drive numbers and pick the boot plan.
    fn resolve(&self) -> Result<(Vec<DiskImage>, PcBootPlan), String> {
        let mut disks = Vec::new();
        for (index, floppy) in self.floppies.iter().enumerate() {
            let drive = u8::try_from(index).map_err(|_| "too many floppies".to_string())?;
            disks.push(DiskImage::floppy(drive, floppy.clone()));
        }
        for (index, disk) in self.hard_disks.iter().enumerate() {
            let drive =
                0x80 + u8::try_from(index).map_err(|_| "too many hard disks".to_string())?;
            disks.push(DiskImage::fixed(drive, disk.clone()));
        }
        if let Some(cdrom) = &self.cdrom {
            disks.push(DiskImage::cdrom(CD_DRIVE, cdrom.clone()));
        }
        if let Some(kernel) = &self.kernel {
            return Ok((disks, PcBootPlan::Linux(kernel.clone())));
        }
        for character in self.boot_order.chars() {
            let plan: Result<Option<PcBootPlan>, String> = match character {
                'a' => Ok(self
                    .floppies
                    .first()
                    .map(|_| PcBootPlan::BootSector { drive: 0x00 })),
                'c' => Ok(self
                    .hard_disks
                    .first()
                    .map(|_| PcBootPlan::BootSector { drive: 0x80 })),
                'd' => match &self.cdrom {
                    Some(cdrom) => Ok(Some(PcBootPlan::ElTorito {
                        image: el_torito_image(cdrom)?,
                        drive: CD_DRIVE,
                    })),
                    None => Ok(None),
                },
                _ => {
                    return Err(format!(
                        "boot order '{}': '{}' is not one of a/c/d",
                        self.boot_order, character
                    ))
                }
            };
            if let Some(plan) = plan? {
                return Ok((disks, plan));
            }
        }
        Err(format!(
            "no bootable image in boot order '{}' (floppies {}, hard disks {}, cdrom {})",
            self.boot_order,
            self.floppies.len(),
            self.hard_disks.len(),
            usize::from(self.cdrom.is_some())
        ))
    }
}

/// The BIOS drive number an El Torito boot CD is conventionally given.
pub const CD_DRIVE: u8 = 0x9F;

/// Where the machine starts executing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootPoint {
    pub cs: u16,
    pub ip: u16,
    pub drive: u8,
    /// Data/stack segment the image expects (`DS`, `ES`, `SS`).
    pub data_segment: u16,
    /// Initial stack pointer.
    pub stack_pointer: u16,
    /// Whether interrupts are enabled on entry. Firmware hands a boot sector
    /// control with `IF` set; a Linux kernel requires it clear.
    pub interrupts: bool,
}

impl BootPoint {
    /// A boot sector at `0000:7C00h`, with the stack just below it.
    pub fn boot_sector(drive: u8) -> Self {
        Self {
            cs: 0x07C0,
            ip: 0,
            drive,
            data_segment: 0,
            stack_pointer: 0x7C00,
            interrupts: true,
        }
    }
}

/// Machine limits and fixtures.
#[derive(Clone, Debug)]
pub struct PcBootOptions {
    /// Instruction budget per quantum.
    pub quantum: u64,
    /// Total instruction budget for the whole run.
    pub instruction_limit: u64,
    /// How many recent instruction addresses to record, for diagnosis. Zero
    /// records nothing; the hook is expensive, so it stays off by default.
    pub trace_instructions: usize,
    /// Guest memory regions to copy into the report after the run, as
    /// `(address, length)`. A stop is often a question about memory content,
    /// and the report is the only thing that survives the run.
    pub dumps: Vec<(u64, usize)>,
    /// Regions whose stores are recorded as `(address, length)`.
    pub watch_writes: Vec<(u64, u64)>,
}

impl Default for PcBootOptions {
    fn default() -> Self {
        Self {
            quantum: 50_000,
            instruction_limit: 200_000_000,
            trace_instructions: 0,
            dumps: Vec::new(),
            watch_writes: Vec::new(),
        }
    }
}

/// Header facts about the kernel that was booted.
#[derive(Clone, Debug)]
pub struct KernelSummary {
    pub version: String,
    pub protocol: String,
    pub setup_sects: u8,
    pub loaded_high: bool,
    pub payload_bytes: usize,
    pub initrd_bytes: usize,
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
    /// Present when the machine booted a Linux kernel rather than a boot
    /// sector or an El Torito image.
    pub kernel: Option<KernelSummary>,
    /// Recent instructions as `(address, size)`, oldest first; empty unless
    /// [`PcBootOptions::trace_instructions`] asked for them.
    pub instruction_trace: Vec<(u64, u32, u64)>,
    /// Registers `EAX..SS` at the end of the run, in Mosaic `RegId` order.
    /// A fault message without the register file is a guessing game.
    pub registers: Vec<u64>,
    /// The backend's own error text for the stop, when it had one.
    pub engine_error: Option<String>,
    /// First unmapped access of the run, as `(is_write, address, size)`. The
    /// error text names the class of fault; this names the pointer.
    pub unmapped: Option<(bool, u64, usize)>,
    /// Bus fault the engine recorded, if the stop was a device refusal.
    pub bus_fault: Option<String>,
    /// Stores into the regions named by [`PcBootOptions::watch_writes`], as
    /// `(guest address, writer eip, size)`, oldest first.
    pub watched_writes: Vec<(u64, u64, u32)>,
    /// Copies of guest memory requested through [`PcBootOptions::dumps`].
    pub dumps: Vec<(u64, Vec<u8>)>,
    /// `CR0` at the end of the run: bit 0 is `PE`, so this says whether the
    /// guest had switched to protected mode before it stopped.
    pub cr0: u64,
    /// The last vector neither the firmware's interrupt service nor the
    /// guest's own gate table could take. The engine delivers refused vectors
    /// through the guest's IDT/IVT itself, so this only fires when the guest
    /// has no handler for the vector.
    pub unclaimed_interrupt: Option<u8>,
    /// The last exception the engine delivered through the guest's gate table,
    /// as `(vector, eip, cr2)`. A kernel that faults and recovers leaves no
    /// other trace: the handler's own `printk` may not be visible yet.
    pub exception: Option<(u8, u64, u64)>,
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
        master: mosaic_exec::NodeId(0),
        security: mosaic_exec::Security::NonSecure,
        privileged: true,
        kind: mosaic_exec::AccessKind::Data,
        exclusive: false,
        byte_enable: u8::MAX,
        endian: mosaic_exec::Endian::Little,
    }
}

/// Serial transmitter state (NS16550D register set, COM1).
///
/// Held by the port router instead of a catalog model because it is the first
/// user: it moves to `hw/src/char/uart_16550` the moment a second machine
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

/// VGA register file, as much of it as a text console needs.
///
/// `vgacon` is not satisfied by a text window alone: before it prints a
/// character it reads the CRTC to learn the cursor position, the row count and
/// the start address, and it programs the attribute controller for colour. A
/// port that reads back as all ones (`FFh`, an idle ISA bus) leaves the console
/// either dead or writing out of the window. Writes are recorded and read back,
/// and the retrace bit alternates so neither retrace poll can spin forever.
#[derive(Debug)]
pub struct VgaRegisters {
    crtc_index: u8,
    crtc: [u8; 32],
    sequencer_index: u8,
    sequencer: [u8; 8],
    graphics_index: u8,
    graphics: [u8; 16],
    attribute_index: u8,
    attribute: [u8; 32],
    retrace: bool,
}

impl Default for VgaRegisters {
    fn default() -> Self {
        let mut vga = Self {
            crtc_index: 0,
            crtc: [0; 32],
            sequencer_index: 0,
            sequencer: [0; 8],
            graphics_index: 0,
            graphics: [0; 16],
            attribute_index: 0,
            attribute: [0; 32],
            retrace: false,
        };
        // The mode 3 state a PC leaves in the CRTC after the video BIOS runs:
        // 80x25 characters of 16 lines each, cursor at the top left, display
        // starting at offset 0 with a 4000-byte line offset.
        vga.crtc[0x0A] = 0x0E; // cursor start scanline
        vga.crtc[0x0B] = 0x0F; // cursor end scanline
        vga.crtc[0x0C] = 0x00; // start address high
        vga.crtc[0x0D] = 0x00; // start address low
        vga.crtc[0x0E] = 0x00; // cursor location high
        vga.crtc[0x0F] = 0x00; // cursor location low
        vga.crtc[0x12] = 0x8F; // vertical display end: 25 * 16 - 1 = 399
        vga.crtc[0x13] = 0x28; // offset: 80 cells per row / 2
        vga
    }
}

impl VgaRegisters {
    fn read(&mut self, port: u16) -> u32 {
        u32::from(match port {
            // Input status 1: the retrace bit alternates. `vgacon` waits for it
            // to clear and then to set again, so a constant answer either way
            // is an infinite loop.
            0x3DA | 0x3BA => {
                self.retrace = !self.retrace;
                if self.retrace {
                    0x09
                } else {
                    0x00
                }
            }
            // Miscellaneous output: colour I/O addressing (bit 0 set), 25 MHz
            // clock, display enabled.
            0x3CC => 0x67,
            0x3C4 => self.sequencer_index,
            0x3C5 => self.sequencer[usize::from(self.sequencer_index) & 0x07],
            0x3CE => self.graphics_index,
            0x3CF => self.graphics[usize::from(self.graphics_index) & 0x0F],
            0x3D4 | 0x3B4 => self.crtc_index,
            0x3D5 | 0x3B5 => self.crtc[usize::from(self.crtc_index) & 0x1F],
            // The attribute controller reads back the index with bit 7 clear.
            0x3C0 | 0x3C1 => self.attribute_index & 0x1F,
            0x3C2 => 0x00,
            _ => 0xFF,
        })
    }

    fn write(&mut self, port: u16, value: u8) {
        match port {
            0x3C4 => self.sequencer_index = value,
            0x3C5 => self.sequencer[usize::from(self.sequencer_index) & 0x07] = value,
            0x3CE => self.graphics_index = value,
            0x3CF => self.graphics[usize::from(self.graphics_index) & 0x0F] = value,
            0x3D4 | 0x3B4 => self.crtc_index = value,
            0x3D5 | 0x3B5 => {
                self.crtc[usize::from(self.crtc_index) & 0x1F] = value;
            }
            0x3C0 | 0x3C1 => {
                // Writing with bit 7 clear after the first write selects the
                // index; the port is shared with the data register.
                if value & 0x80 != 0 {
                    self.attribute_index = value & 0x1F;
                } else if self.attribute_index < 0x20 {
                    self.attribute[usize::from(self.attribute_index)] = value;
                }
            }
            _ => {}
        }
    }
}

/// PC port space: the only way a real guest reaches these devices.
/// Channel-0 timer shadow: the divisor and mode the guest programmed into
/// the PIT, snooped on the port path, so the machine can convert executed
/// instructions into IRQ-0 edges. A real machine wires the PIT output to the
/// PIC input; until the clock tree (ADR 0003's resolved clock) reaches this
/// interim, this shadow is the wire, and the catalog PIT model keeps serving
/// register reads exactly as before.
#[derive(Debug, Default)]
struct PitIrqShadow {
    /// Divider written to channel 0 (0 means 65536).
    divisor: u32,
    /// Nanoseconds of channel-0 countdown left until the next edge.
    countdown_ns: u64,
    /// Which half of the two-byte divisor write sequence is next.
    lobyte: bool,
}

impl PitIrqShadow {
    /// One instruction is one tick of a nominal 100 MHz core. The ratio only
    /// has to be *consistent* for a deterministic guest; the kernel derives
    /// everything it needs from the edges themselves.
    const NS_PER_INSTRUCTION: u64 = 10;

    fn observe_write(&mut self, port: u16, value: u8) {
        match port {
            0x43 => {
                // Command: access mode bits say how the next channel-0 count
                // write arrives; latch-only commands leave the divisor alone.
                let access = (value >> 4) & 0x3;
                self.lobyte = access == 0;
            }
            0x40 => {
                let access_lobyte = self.lobyte;
                self.lobyte = !self.lobyte;
                let divisor = self.divisor;
                self.divisor = match (access_lobyte, divisor) {
                    // Both halves arrive; the second byte completes them.
                    (true, _) => u32::from(value),
                    (false, high) => (high & 0xFF) | (u32::from(value) << 8),
                };
                // A new period starts from the write.
                self.reload();
            }
            _ => {}
        }
    }

    fn reload(&mut self) {
        let divisor = if self.divisor == 0 {
            65_536
        } else {
            self.divisor
        };
        // The PIT's input clock is 1 193 182 Hz: one count is 838 ns.
        self.countdown_ns = u64::from(divisor) * 838;
    }

    /// Advance by the instructions the last quantum retired, returning the
    /// number of channel-0 edges that fired.
    fn advance(&mut self, instructions: u64) -> u32 {
        let mut nanos = instructions.saturating_mul(Self::NS_PER_INSTRUCTION);
        let mut edges = 0u32;
        while nanos > 0 && self.countdown_ns > 0 {
            if nanos >= self.countdown_ns {
                nanos -= self.countdown_ns;
                self.countdown_ns = 0;
                self.reload();
                edges += 1;
            } else {
                self.countdown_ns -= nanos;
                nanos = 0;
            }
        }
        edges
    }
}

struct PcPorts {
    pic: Arc<Mutex<Pic8259>>,
    pit: Pit8254,
    /// Channel-0 IRQ shadow, shared with the machine loop.
    timer: Arc<Mutex<PitIrqShadow>>,
    rtc: RtcMc146818,
    serial: Arc<Mutex<Serial16550>>,
    vga: VgaRegisters,
}

impl PortBus for PcPorts {
    fn read(&mut self, port: u16, _size: usize) -> Result<u32, BusFault> {
        let mut ctx = FirmwareCtx;
        let attr = word_attr();
        match port {
            0x20..=0x21 => {
                let offset = u64::from(port - 0x20);
                let mut pic = self.pic.lock().map_err(|_| BusFault::Device)?;
                let value = pic.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
                Ok((value & 0xFF) as u32)
            }
            0xA0..=0xA1 => {
                let offset = u64::from(port - 0xA0) + 2;
                let mut pic = self.pic.lock().map_err(|_| BusFault::Device)?;
                let value = pic.read(&mut ctx, offset, AccessSize::Byte, &attr)?;
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
            // The VGA's register file. A text console lives or dies by these.
            0x3B4..=0x3B5
            | 0x3BA
            | 0x3C0..=0x3C2
            | 0x3C4..=0x3C5
            | 0x3CC
            | 0x3CE..=0x3CF
            | 0x3D4..=0x3D5
            | 0x3DA => Ok(self.vga.read(port)),
            // 8042 keyboard controller status: no byte waiting, input buffer
            // empty. Firmware flushes this port before enabling A20 and
            // before every mode switch, and a status byte of all ones (an
            // unclaimed port) makes that flush loop forever.
            0x64 => Ok(0x00),
            // POST diagnostic port: writes are dropped, reads see nothing.
            0x80 => Ok(0x00),
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
                let mut pic = self.pic.lock().map_err(|_| BusFault::Device)?;
                pic.write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0xA0..=0xA1 => {
                let offset = u64::from(port - 0xA0) + 2;
                let mut pic = self.pic.lock().map_err(|_| BusFault::Device)?;
                pic.write(&mut ctx, offset, AccessSize::Byte, u64::from(byte), &attr)
            }
            0x40..=0x43 => {
                let offset = u64::from(port - 0x40);
                if let Ok(mut timer) = self.timer.lock() {
                    timer.observe_write(port, byte);
                }
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
            0x3B4..=0x3B5 | 0x3C0..=0x3C2 | 0x3C4..=0x3C5 | 0x3CE..=0x3CF | 0x3D4..=0x3D5 => {
                self.vga.write(port, byte);
                Ok(())
            }
            // The 8042 data port and the POST diagnostic port take writes as
            // completion tokens: nothing is attached, and firmware writes them
            // to time a bus cycle, not to control a device.
            0x60 | 0x64 | 0x80 => Ok(()),
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
    /// Extended RAM above 1 MiB, in KiB: what `INT 15h AH=88h`, `E801h` and
    /// the `E820h` map report. The machine's own mapping, not a constant.
    extended_kib: u64,
    /// The El Torito boot image the machine booted from, if it booted a CD.
    eltorito: Option<ElToritoImage>,
    /// First vector the firmware could not answer.
    unhandled: Option<u8>,
    /// `(vector, cs, ip)` for every firmware call, in order.
    log: Vec<(u8, u64, u64)>,
}

impl PcBios {
    fn new(
        disks: Vec<DiskImage>,
        boot_drive: u8,
        extended_kib: u64,
        eltorito: Option<ElToritoImage>,
    ) -> Self {
        Self {
            disks,
            cursor_row: 0,
            cursor_col: 0,
            interrupts: BTreeMap::new(),
            disk_reads: Vec::new(),
            boot_drive,
            extended_kib,
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

    /// Linear base of a selector.
    ///
    /// In real mode a selector is a paragraph address. Once the guest has set
    /// `CR0.PE` the same register holds a descriptor index, and the guest's
    /// own descriptors (flat, base 0) decide the base — firmware that kept
    /// multiplying by 16 would write its answers to paragraph-sized addresses
    /// and the guest would read garbage.
    fn segment_base(cpu: &dyn SoftwareInterruptCpu, selector: u16) -> u64 {
        let protected = cpu
            .reg_read(REG_CR0)
            .map(|cr0| cr0 & 1 != 0)
            .unwrap_or(false);
        if protected {
            0
        } else {
            u64::from(selector) * 16
        }
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
                Self::set_ax(cpu, ax & 0xFF00);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0x02 => self.chs_read(cpu, drive),
            0x08 => {
                // Geometry. A bootloader divides by these numbers, so every
                // one of them must match the media: DL = drive count, DH = last
                // head index, CL = last sector, CH = last cylinder.
                if let Some(disk) = self.disk(drive) {
                    let (heads, sectors) = disk.geometry();
                    let cylinders = (disk.blocks() / (heads * sectors)).min(1023);
                    let cl = 0xC0 & (cylinders >> 2) as u8 | sectors as u8;
                    let ch = (cylinders & 0xFF) as u8;
                    let _ = cpu.reg_write(REG_ECX, (u64::from(ch) << 8) | u64::from(cl));
                    Self::set_ax(cpu, ax & 0xFF00);
                    let _ = cpu.reg_write(REG_EDX, ((heads - 1) << 8) | 0x01);
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
        let buffer = Self::segment_base(cpu, cpu.reg_read(REG_DS).unwrap_or(0) as u16)
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
        let buffer = Self::segment_base(cpu, cpu.reg_read(REG_ES).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_EBX).unwrap_or(0) as u16);
        let Some(disk) = self.disk(drive).cloned() else {
            Self::set_ax(cpu, (ax & 0xFF00) | 0x01);
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        };
        let sectors_per_track = disk.geometry().1;
        let heads = disk.geometry().0;
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
        let packet = Self::segment_base(cpu, cpu.reg_read(REG_DS).unwrap_or(0) as u16)
            + u64::from(cpu.reg_read(REG_ESI).unwrap_or(0) as u32);
        let count = Self::read_word(cpu, packet + 2);
        let offset = Self::read_word(cpu, packet + 4);
        let segment = Self::read_word(cpu, packet + 6);
        let lba = u64::from(Self::read_dword(cpu, packet + 8))
            | (u64::from(Self::read_dword(cpu, packet + 12)) << 32);
        let buffer = Self::segment_base(cpu, segment) + u64::from(offset);
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
        let packet = Self::segment_base(cpu, cpu.reg_read(REG_DS).unwrap_or(0) as u16)
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
                // Extended memory in KiB. The interface caps at 15 MiB, so a
                // machine with more reports the cap and leaves the rest to
                // `E801h` and `E820h`, which is what real firmware does.
                Self::set_ax(cpu, (self.extended_kib).min(0x3C00) as u16);
                Self::set_flag(cpu, FLAG_CF, false);
            }
            0xE801 => {
                // 1 MiB..16 MiB in KiB, then 16 MiB..4 GiB in 64 KiB pages.
                let below = self.extended_kib.min(0x3C00);
                let above = (self.extended_kib.saturating_sub(0x3C00)) / 64;
                let _ = cpu.reg_write(REG_EAX, below);
                let _ = cpu.reg_write(REG_EBX, above);
                let _ = cpu.reg_write(REG_ECX, below);
                let _ = cpu.reg_write(REG_EDX, above);
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
            (0x0010_0000, self.extended_kib * 1024, 1),
        ];
        let mut ebx = cpu.reg_read(REG_EBX).unwrap_or(0) as u32;
        if ebx as usize >= entries.len() {
            Self::set_flag(cpu, FLAG_CF, true);
            return;
        }
        let buffer = Self::segment_base(cpu, cpu.reg_read(REG_ES).unwrap_or(0) as u16)
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
                let buffer = Self::segment_base(cpu, cpu.reg_read(REG_ES).unwrap_or(0) as u16)
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
                // Equipment word: initial video mode 80x25 colour (bit 5),
                // and the floppy count the machine actually has: bit 0 says
                // any, bits 7..6 hold the count minus one.
                let floppies = self
                    .disks
                    .iter()
                    .filter(|disk| disk.media == DriveMedia::Floppy)
                    .count()
                    .min(4) as u16;
                let mut word = 0x0020u16;
                if floppies > 0 {
                    word |= 1 | ((floppies - 1) << 6);
                }
                Self::set_ax(cpu, word);
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
            // IRQ proxies and the firmware's other no-op vectors: acknowledged,
            // with no hardware behind them yet. Vector 8 is deliberately *not*
            // in this list even though it is IRQ0: the same vector is `#DF`,
            // and every vector below 32 belongs to the CPU, which the engine
            // delivers through the guest's own gate table.
            0x1B | 0x1C | 0x4A | 0x70 | 0x71 | 0x75 => {}
            // INT 19h is the firmware's own bootstrap; the runner performs it
            // before the first quantum, so a guest reaching it means the boot
            // image gave up.
            0x19 => {
                return false;
            }
            // Everything else — every CPU exception (0..32), and any firmware
            // call this machine does not implement — is left to the guest's own
            // gate table. The engine delivers it; only a vector with no usable
            // gate stops the quantum.
            _ => {
                self.unhandled = Some(vector);
                return false;
            }
        }
        true
    }
}

/// Bytes a real firmware would have left in low memory before the boot
/// image runs: the interrupt vector table, the BIOS data area and the
/// trampolines the vectors point at. (The El Torito extraction itself lives in
/// `mosaic-modelkit-iso9660`; this is the machine's use of its result.)
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

/// Boot the configured PC and run it. This is the machine's whole
/// front-facing surface: RAM size, drives, boot order, an optional direct
/// kernel — the same facts QEMU's own command line names.
pub fn run_pc_machine(
    config: &PcMachineConfig,
    options: &PcBootOptions,
) -> Result<PcBootReport, String> {
    let (disks, plan) = config.resolve()?;
    // Extended RAM is everything `-m` bought above the low megabyte, and the
    // machine maps exactly as much as it reports through `INT 15h`.
    let extended_kib = config
        .ram_kib
        .saturating_sub(1024)
        .clamp(0, MAX_EXTENDED_KIB);
    match plan {
        PcBootPlan::ElTorito { image, drive } => {
            let boot = BootPoint {
                cs: image.load_segment,
                ip: 0,
                drive,
                data_segment: 0,
                stack_pointer: 0x7C00,
                interrupts: true,
            };
            run_with(
                disks,
                boot,
                options,
                Some(image),
                None,
                extended_kib,
                config.cpu_model,
            )
        }
        PcBootPlan::BootSector { drive } => {
            let boot = BootPoint::boot_sector(drive);
            run_with(
                disks,
                boot,
                options,
                None,
                None,
                extended_kib,
                config.cpu_model,
            )
        }
        PcBootPlan::Linux(kernel) => {
            run_linux_machine(&disks, &kernel, options, extended_kib, config.cpu_model)
        }
    }
}

/// Direct kernel boot: place the `bzImage` the way the i386 boot protocol
/// documents, and hand control to the kernel's own real-mode entry.
fn run_linux_machine(
    disks: &[DiskImage],
    kernel: &DirectKernel,
    options: &PcBootOptions,
    extended_kib: u64,
    cpu_model: mosaic_accel_unicorn::X86CpuModel,
) -> Result<PcBootReport, String> {
    let parsed = mosaic_boot::proto::x86_linux::BzImage::parse(&kernel.image)?;
    let initrd = kernel.initrd.as_ref().map(|bytes| {
        // Above the kernel, page aligned: the kernel relocates nothing that a
        // loader put there.
        let base = (parsed.payload_base() + parsed.payload.len() as u64 + 0xFFFF) & !0xFFFF;
        (base, bytes.clone())
    });
    let mut setup = parsed.setup.clone();
    let command = mosaic_boot::proto::x86_linux::prepare_setup(
        &mut setup,
        &parsed,
        &mosaic_boot::proto::x86_linux::BootParams {
            cmdline: kernel.cmdline.clone(),
            initrd: initrd
                .as_ref()
                .map(|(base, bytes)| (*base, bytes.len() as u64)),
        },
        extended_kib,
    )?;
    let mut blobs: Vec<(u64, Vec<u8>)> = initrd.iter().cloned().collect();
    blobs.push((
        mosaic_boot::proto::x86_linux::command_line_address(),
        command,
    ));
    let placement = KernelPlacement {
        setup,
        payload: parsed.payload.clone(),
        payload_base: parsed.payload_base(),
        blobs,
    };
    let boot = BootPoint {
        cs: parsed.entry_segment(),
        ip: 0,
        drive: 0x80,
        data_segment: (SETUP_BASE >> 4) as u16,
        stack_pointer: mosaic_boot::proto::x86_linux::HEAP_END,
        interrupts: false,
    };
    let mut report = run_with(
        disks.to_vec(),
        boot,
        options,
        None,
        Some(placement),
        extended_kib,
        cpu_model,
    )?;
    report.kernel = Some(KernelSummary {
        version: parsed.version.clone(),
        protocol: format!("{:#06x}", parsed.protocol),
        setup_sects: parsed.setup_sects,
        loaded_high: parsed.loaded_high(),
        payload_bytes: parsed.payload.len(),
        initrd_bytes: initrd.as_ref().map(|(_, bytes)| bytes.len()).unwrap_or(0),
    });
    Ok(report)
}

/// Where a Linux kernel's two halves go.
pub struct KernelPlacement {
    pub setup: Vec<u8>,
    pub payload: Vec<u8>,
    pub payload_base: u64,
    /// Extra images at fixed addresses, such as an initramfs.
    pub blobs: Vec<(u64, Vec<u8>)>,
}

fn run_with(
    disks: Vec<DiskImage>,
    boot: BootPoint,
    options: &PcBootOptions,
    image: Option<ElToritoImage>,
    kernel: Option<KernelPlacement>,
    extended_kib: u64,
    cpu_model: mosaic_accel_unicorn::X86CpuModel,
) -> Result<PcBootReport, String> {
    let mut engine = UnicornX86Boot::new_as(cpu_model)
        .map_err(|error| format!("engine (cpu model {cpu_model:?}): {error:?}"))?;
    // Conventional RAM, the VGA text window and the BIOS window are all real
    // guest memory: the firmware writes the screen into the same window the
    // guest reads.
    engine
        .map_memory(
            mosaic_exec::Addr(0),
            0xA_0000,
            mosaic_exec::Permissions::RWX,
        )
        .map_err(|error| format!("ram map: {error:?}"))?;
    // The whole `A0000h..BFFFFh` aperture, not just the text window: the
    // firmware and the kernel both read it (the early ACPI scan walks
    // `E0000h..FFFFFh`, and video probing reads the graphics half).
    engine
        .map_memory(
            mosaic_exec::Addr(VGA_APERTURE_BASE),
            VGA_APERTURE_SIZE,
            mosaic_exec::Permissions::RWX,
        )
        .map_err(|error| format!("vga map: {error:?}"))?;
    // The `C0000h..FFFFFh` ROM window. The BIOS image occupies its top 64 KiB;
    // the space below it decodes to zeros, which is what a real board does and
    // what the kernel's RSDP scan at `E0000h` expects to read.
    engine
        .map_memory(
            mosaic_exec::Addr(ROM_GAP_BASE),
            ROM_GAP_SIZE,
            mosaic_exec::Permissions::RWX,
        )
        .map_err(|error| format!("rom map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_exec::Addr(BIOS_ROM_BASE),
            BIOS_ROM_SIZE,
            mosaic_exec::Permissions::RWX,
        )
        .map_err(|error| format!("bios map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_exec::Addr(0x0010_0000),
            extended_kib * 1024,
            mosaic_exec::Permissions::RWX,
        )
        .map_err(|error| format!("extended ram map: {error:?}"))?;
    engine
        .map_memory(
            mosaic_exec::Addr(LOCAL_APIC_BASE),
            LOCAL_APIC_SIZE,
            mosaic_exec::Permissions::RW,
        )
        .map_err(|error| format!("lapic map: {error:?}"))?;

    let serial = Arc::new(Mutex::new(Serial16550::default()));
    let pic = Arc::new(Mutex::new(Pic8259::new()));
    let timer = Arc::new(Mutex::new(PitIrqShadow::default()));
    engine
        .attach_ports(Box::new(PcPorts {
            pic: Arc::clone(&pic),
            pit: Pit8254::new(),
            timer: Arc::clone(&timer),
            rtc: RtcMc146818::new(),
            serial: Arc::clone(&serial),
            vga: VgaRegisters::default(),
        }))
        .map_err(|error| format!("ports: {error:?}"))?;

    // A raw disk contributes its first sector to `7C00h`, which is where the
    // firmware's own bootstrap would have copied it. The sector belongs to
    // the drive the boot plan named, not to whichever drive is first.
    let boot_sector = if image.is_none() && kernel.is_none() {
        let disk = disks
            .iter()
            .find(|disk| disk.drive == boot.drive)
            .ok_or("boot disk missing")?;
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
        .write_memory(mosaic_exec::Addr(0), firmware.low_memory())
        .map_err(|error| format!("firmware low memory: {error:?}"))?;
    engine
        .write_memory(mosaic_exec::Addr(BIOS_ROM_BASE), firmware.rom())
        .map_err(|error| format!("firmware rom: {error:?}"))?;

    let bios = Arc::new(Mutex::new(PcBios::new(
        disks,
        boot.drive,
        extended_kib,
        image.clone(),
    )));
    engine
        .attach_interrupt_service(Box::new(SharedBios {
            inner: Arc::clone(&bios),
        }))
        .map_err(|error| format!("interrupt service: {error:?}"))?;

    // Load the boot image where the boot record says it lives.
    if let Some(image) = &image {
        let base = u64::from(image.load_segment) * 16;
        engine
            .write_memory(mosaic_exec::Addr(base), &image.bytes)
            .map_err(|error| format!("boot image load: {error:?}"))?;
    }
    if let Some(kernel) = &kernel {
        engine
            .write_memory(mosaic_exec::Addr(SETUP_BASE), &kernel.setup)
            .map_err(|error| format!("kernel setup load: {error:?}"))?;
        engine
            .write_memory(mosaic_exec::Addr(kernel.payload_base), &kernel.payload)
            .map_err(|error| format!("kernel payload load: {error:?}"))?;
        for (base, bytes) in &kernel.blobs {
            engine
                .write_memory(mosaic_exec::Addr(*base), bytes)
                .map_err(|error| format!("kernel blob at {base:#x}: {error:?}"))?;
        }
    }
    if let Some(sector) = &boot_sector {
        engine
            .write_memory(mosaic_exec::Addr(0x7C00), sector)
            .map_err(|error| format!("boot sector load: {error:?}"))?;
    }

    // `JMP FAR seg:offset` to the boot point (see `BOOT_TRAMPOLINE`).
    let mut trampoline = vec![0xEA];
    trampoline.extend_from_slice(&boot.ip.to_le_bytes());
    trampoline.extend_from_slice(&boot.cs.to_le_bytes());
    engine
        .write_memory(mosaic_exec::Addr(BOOT_TRAMPOLINE), &trampoline)
        .map_err(|error| format!("trampoline: {error:?}"))?;

    use mosaic_isa_x86::i386::Register;
    engine
        .reg_write(Register::Cs.into(), 0)
        .map_err(|error| format!("cs: {error:?}"))?;
    engine
        .reg_write(Register::Eip.into(), BOOT_TRAMPOLINE)
        .map_err(|error| format!("eip: {error:?}"))?;
    engine
        .reg_write(Register::Ds.into(), u64::from(boot.data_segment))
        .map_err(|error| format!("ds: {error:?}"))?;
    engine
        .reg_write(Register::Es.into(), u64::from(boot.data_segment))
        .map_err(|error| format!("es: {error:?}"))?;
    engine
        .reg_write(Register::Ss.into(), u64::from(boot.data_segment))
        .map_err(|error| format!("ss: {error:?}"))?;
    engine
        .reg_write(Register::Esp.into(), u64::from(boot.stack_pointer))
        .map_err(|error| format!("sp: {error:?}"))?;
    engine
        .reg_write(Register::Edx.into(), u64::from(boot.drive))
        .map_err(|error| format!("dl: {error:?}"))?;
    engine
        .reg_write(
            Register::Eflags.into(),
            if boot.interrupts { 0x202 } else { 0x002 },
        )
        .map_err(|error| format!("flags: {error:?}"))?;

    for (base, length) in &options.watch_writes {
        engine
            .watch_writes(*base, *length)
            .map_err(|error| format!("watch: {error:?}"))?;
    }
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
        // The virtual PIT: retired instructions become nanoseconds, edges
        // become IRQ-0 raises the way the wire would carry them. The PIC
        // decides (mask, priority) and names the vector; the engine delivers
        // it through the guest's own IDT at the next quantum.
        let edges = timer
            .lock()
            .map(|mut shadow| shadow.advance(outcome.instruction_hooks))
            .unwrap_or(0);
        for _ in 0..edges {
            // Level semantics: while one IRQ-0 raise is still waiting for the
            // guest to open `IF`, later edges fold into it — a guest that
            // programmed a fast tick must not drown in its own timer, and a
            // guest that never opens `IF` (a boot sector that only polls)
            // keeps exactly one pending raise, exactly like a PIC wired to a
            // core that masks interrupts.
            if engine.pending_interrupts() > 0 {
                break;
            }
            let vector = pic.lock().map(|mut pic| {
                let mut ctx = FirmwareCtx;
                pic.set_irq(&mut ctx, 0, true);
                let vector = pic.acknowledge(&mut ctx);
                pic.set_irq(&mut ctx, 0, false);
                vector
            });
            if let Ok(vector) = vector {
                // The only source this machine raises is IRQ 0; a spurious
                // acknowledge (masked or wrong state) answers icw2+7 and is
                // dropped, never injected.
                if vector & 0x07 == 0 {
                    engine.queue_interrupt(vector);
                }
            }
        }
        if outcome.reason != StopReason::BudgetExhausted {
            break;
        }
    }

    let last_traced = engine
        .instruction_trace()
        .last()
        .map(|(address, _, _)| *address);
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
        kernel: None,
        instruction_trace: engine.instruction_trace(),
        watched_writes: engine.watched_writes(),
        dumps: options
            .dumps
            .iter()
            .map(|(address, length)| {
                let mut bytes = vec![0u8; *length];
                let ok = engine
                    .read_memory(mosaic_exec::Addr(*address), &mut bytes)
                    .is_ok();
                (*address, if ok { bytes } else { Vec::new() })
            })
            .collect(),
        registers: (0..16u32)
            .map(|id: u32| engine.reg_read(RegId(id)).unwrap_or(0))
            .collect(),
        engine_error: engine.last_error(),
        unmapped: engine.take_unmapped(),
        bus_fault: engine.take_bus_fault().map(|fault| format!("{fault:?}")),
        cr0: engine
            .reg_read(mosaic_isa_x86::i386::Register::Cr0.into())
            .unwrap_or(0),
        unclaimed_interrupt: engine.take_unclaimed_interrupt(),
        exception: engine.take_exception(),
        last_instruction: {
            let mut bytes = [0u8; 16];
            match last_traced {
                Some(address)
                    if engine
                        .read_memory(mosaic_exec::Addr(address), &mut bytes)
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
    engine: &'a UnicornX86Boot,
}

impl SoftwareInterruptCpu for FirmwareCpuView<'_> {
    fn reg_read(&self, reg: RegId) -> Result<u64, mosaic_contract::cpu_engine::EngineError> {
        CpuEngine::reg_read(self.engine, reg)
    }

    fn reg_write(
        &mut self,
        _reg: RegId,
        _value: u64,
    ) -> Result<(), mosaic_contract::cpu_engine::EngineError> {
        Err(mosaic_contract::cpu_engine::EngineError::InvalidRegister)
    }

    fn mem_read(
        &self,
        address: u64,
        data: &mut [u8],
    ) -> Result<(), mosaic_contract::cpu_engine::EngineError> {
        CpuEngine::read_memory(self.engine, mosaic_exec::Addr(address), data)
    }

    fn mem_write(
        &mut self,
        _address: u64,
        _data: &[u8],
    ) -> Result<(), mosaic_contract::cpu_engine::EngineError> {
        Err(mosaic_contract::cpu_engine::EngineError::Unmapped)
    }
}

/// Unused today, kept so `VDelta` stays imported for the delay services that
/// arrive with the PIT-backed clock.
const _: fn(VDelta) -> VTime = |delay| VTime::ZERO.checked_add(delay).unwrap_or(VTime::ZERO);
