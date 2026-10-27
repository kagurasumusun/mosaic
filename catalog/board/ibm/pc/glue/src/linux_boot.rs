//! Booting a real Linux kernel on the `pc` machine: the i386 boot protocol.
//!
//! This is the machine's `-kernel` path — the QEMU `hw/i386/x86.c`
//! `x86_load_linux()` role, not a machine of its own and not front-end code.
//! `-kernel`/`-initrd`/`-append` are machine-neutral words the front-end
//! parses (QEMU stores them in `MachineState`); what they *mean* on a given
//! machine is the machine's boot protocol, and this module is the PC's. When
//! a second x86 machine arrives (`q35`, ...) this module promotes to a shared
//! modelkit crate unchanged, the way `x86_load_linux` serves every x86
//! machine type (design §4 Rule of two).
//!
//! A Linux CD is two problems stacked on each other. The first is the boot
//! loader (isolinux), which reads the kernel file off the ISO and hands control
//! to it; the second is the kernel's own real-mode `setup` code, which builds
//! the machine state a protected-mode kernel expects. This module takes the
//! second half seriously: it *is* the boot loader for a `bzImage`, following
//! the documented protocol rather than guessing, so the guest that runs next is
//! the kernel's own decompressor and its own 32-bit entry code. (Pulling a file
//! out of an ISO is a format concern and lives in `mosaic-modelkit-iso9660`.)
//!
//! Clean-room from `Documentation/arch/x86/boot.rst`: the header at offset
//! `0x1F1`, the real-mode part in the low 32 KiB of the file, the 32-bit part
//! at `(setup_sects + 1) * 512` and, for a `LOAD_HIGH` kernel, at `100000h`.

/// Where the real-mode part of a Linux kernel is loaded.
pub const SETUP_BASE: u64 = 0x0009_0000;
/// Where a `LOAD_HIGH` kernel's protected-mode part is loaded.
pub const PAYLOAD_HIGH: u64 = 0x0010_0000;
/// Where a plain `zImage`'s protected-mode part is loaded.
pub const PAYLOAD_LOW: u64 = 0x0001_0000;
/// Top of the heap the kernel's `setup` code is told about, relative to
/// [`SETUP_BASE`]; the command line lives at the very top of that segment.
pub const HEAP_END: u16 = 0xE000;
/// `boot_params` field: extended boot loader identifier (`0xF0` = Mosaic).
pub const TYPE_OF_LOADER: u8 = 0xF0;
/// `loadflags` bit 0: the kernel is loaded above 1 MiB.
const LOADED_HIGH: u8 = 0x01;
/// `loadflags` bit 7: the kernel may use the heap at `heap_end_ptr`.
const CAN_USE_HEAP: u8 = 0x80;
/// Offset of the 64-bit entry point inside the protected-mode payload.
pub const ENTRY_64_OFFSET: u64 = 0x200;
/// `screen_info.orig_video_isVGA` for a VGA in text mode, which is what the
/// machine's text window at `B8000h` is.
pub const VIDEO_TYPE_VGAC: u8 = 0x22;
/// Size of the RAM above 1 MiB a default machine reports, in KiB. A machine
/// configured with `-m` passes its own number through to `prepare_setup`.
pub const DEFAULT_EXTENDED_KIB: u64 = 63 * 1024;

/// A parsed `bzImage`: the real-mode part and the protected-mode part, plus the
/// header fields a boot loader is required to honour.
#[derive(Clone, Debug)]
pub struct BzImage {
    pub setup_sects: u8,
    pub protocol: u16,
    pub loadflags: u8,
    pub setup: Vec<u8>,
    pub payload: Vec<u8>,
    pub version: String,
}

impl BzImage {
    /// Parse a kernel image. Anything that is not a `bzImage` with a `HdrS`
    /// header is rejected rather than guessed at.
    pub fn parse(kernel: &[u8]) -> Result<Self, String> {
        if kernel.len() < 0x220 {
            return Err("kernel image is too small to hold a header".into());
        }
        let magic =
            u32::from_le_bytes([kernel[0x202], kernel[0x203], kernel[0x204], kernel[0x205]]);
        if magic != 0x5372_6448 {
            return Err(format!(
                "not a bzImage: header magic is {magic:#010x}, not 'HdrS'"
            ));
        }
        let protocol = u16::from_le_bytes([kernel[0x206], kernel[0x207]]);
        if protocol < 0x0200 {
            return Err(format!(
                "boot protocol {protocol:#06x} is too old to describe"
            ));
        }
        let loadflags = kernel[0x211];
        let setup_sects = match kernel[0x1F1] {
            0 => 4,
            value => value,
        };
        let setup_len = (usize::from(setup_sects) + 1) * 512;
        if kernel.len() < setup_len {
            return Err("kernel image is shorter than its setup code".into());
        }
        // The version string pointer is relative to the start of the setup
        // code and points 0x200 bytes before the header.
        let version_ptr = u16::from_le_bytes([kernel[0x20E], kernel[0x20F]]) as usize;
        let version = if version_ptr == 0 {
            String::new()
        } else {
            let start = 0x200usize.saturating_add(version_ptr);
            let end = kernel[start..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|offset| start + offset)
                .unwrap_or(start);
            String::from_utf8_lossy(&kernel[start.min(end)..end]).to_string()
        };
        Ok(Self {
            setup_sects,
            protocol,
            loadflags,
            setup: kernel[..setup_len].to_vec(),
            payload: kernel[setup_len..].to_vec(),
            version,
        })
    }

    /// Whether the protected-mode part belongs above 1 MiB.
    pub fn loaded_high(&self) -> bool {
        self.loadflags & LOADED_HIGH != 0
    }

    /// Where the protected-mode part must be loaded.
    pub fn payload_base(&self) -> u64 {
        if self.loaded_high() {
            PAYLOAD_HIGH
        } else {
            PAYLOAD_LOW
        }
    }

    /// Real-mode segment the boot loader jumps into: `setup` code plus `0x20`.
    pub fn entry_segment(&self) -> u16 {
        (SETUP_BASE >> 4) as u16 + 0x20
    }
}

/// Edits a boot loader must make inside `boot_params` (the setup area) before
/// handing control to the kernel.
#[derive(Clone, Debug, Default)]
pub struct BootParams {
    /// Kernel command line.
    pub cmdline: String,
    /// Already-loaded initramfs: `(address, size)`.
    pub initrd: Option<(u64, u64)>,
}

/// Rewrite the header fields a boot loader owns, and write the command line and
/// the memory map into the setup area.
///
/// `setup` is the kernel's own real-mode image and doubles as `struct
/// boot_params`: the kernel reads it back at `SETUP_BASE`, so every value the
/// protocol says the loader must supply is written *into the copy that will be
/// in guest memory*.
///
/// Returns the command line bytes; the caller places them at
/// [`command_line_address`], which is above the setup image itself.
pub fn prepare_setup(
    setup: &mut [u8],
    kernel: &BzImage,
    params: &BootParams,
    extended_kib: u64,
) -> Result<Vec<u8>, String> {
    if setup.len() < 0x1000 {
        return Err("setup area is shorter than one page".into());
    }
    // Loader identity, and permission to use the heap.
    setup[0x210] = TYPE_OF_LOADER;
    setup[0x211] = kernel.loadflags | CAN_USE_HEAP;
    let heap_end_ptr = HEAP_END - 0x200;
    setup[0x224..0x226].copy_from_slice(&heap_end_ptr.to_le_bytes());
    // The command line sits at the top of the real-mode segment.
    let mut command = params.cmdline.clone().into_bytes();
    command.push(0);
    // The kernel's own limit for protocol 2.06+ is `cmdline_size`; older
    // kernels were limited to 255 bytes.
    setup[0x228..0x22C].copy_from_slice(&(command_line_address() as u32).to_le_bytes());
    if let Some((address, size)) = params.initrd {
        setup[0x218..0x21C].copy_from_slice(&(address as u32).to_le_bytes());
        setup[0x21C..0x220].copy_from_slice(&(size as u32).to_le_bytes());
    }
    // The memory map is *not* written here. `struct boot_params` at 0x2D0 is
    // inside the real-mode image the kernel is executing from, and the kernel
    // builds that table itself by calling `INT 15h` with `E820` — which this
    // machine answers (see `PcBios`). Writing it from the loader overwrites
    // the kernel's own `intcall` routine, and the kernel then executes the
    // memory map as if it were code.
    // Screen: `0xFFFF` is the protocol's "normal video", meaning "leave the
    // display the way the firmware set it up". Asking for a specific mode
    // number makes the kernel probe the VGA BIOS mode table, which this
    // firmware does not publish, and it stops to ask the user instead.
    setup[0x1FA..0x1FC].copy_from_slice(&0xFFFFu16.to_le_bytes());
    // `screen_info` at the head of `boot_params`. This is the loader's job, and
    // it is what `vgacon` reads before it will offer itself as a console: with
    // `orig_video_isVGA = 0` the kernel registers no `tty0` at all and every
    // `printk` of the boot goes nowhere. The machine already writes text into
    // `B8000h`, so the loader describes exactly that: 80x25, colour, 16-pixel
    // font, cursor at the top left.
    setup[0x00] = 0; // orig_x
    setup[0x01] = 0; // orig_y
    setup[0x02..0x04].copy_from_slice(&(extended_kib.min(0xFFFF) as u16).to_le_bytes()); // ext_mem_k
    setup[0x04..0x06].copy_from_slice(&0u16.to_le_bytes()); // orig_video_page
    setup[0x06] = 3; // orig_video_mode: 80x25 colour text
    setup[0x07] = 80; // orig_video_cols
    setup[0x08] = 0; // flags
    setup[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes()); // orig_video_ega_bx
    setup[0x0E] = 25; // orig_video_lines
    setup[0x0F] = VIDEO_TYPE_VGAC; // orig_video_isVGA
    setup[0x10..0x12].copy_from_slice(&16u16.to_le_bytes()); // orig_video_points
    Ok(command)
}

/// Address the kernel reads its command line from: the top of the real-mode
/// segment, above the setup image and the heap.
pub fn command_line_address() -> u64 {
    SETUP_BASE + u64::from(HEAP_END)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_without_the_magic_is_rejected() {
        let kernel = vec![0u8; 0x1000];
        assert!(BzImage::parse(&kernel).is_err());
    }
}
