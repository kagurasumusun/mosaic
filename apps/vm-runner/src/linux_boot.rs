//! Booting a real Linux kernel on the PC machine: the i386 boot protocol.
//!
//! A Linux CD is two problems stacked on each other. The first is the boot
//! loader (isolinux), which reads the kernel file off the ISO and hands control
//! to it; the second is the kernel's own real-mode `setup` code, which builds
//! the machine state a protected-mode kernel expects. This module takes the
//! second half seriously: it *is* the boot loader for a `bzImage`, following
//! the documented protocol rather than guessing, so the guest that runs next is
//! the kernel's own decompressor and its own 32-bit entry code.
//!
//! Clean-room from `Documentation/arch/x86/boot.rst`: the header at offset
//! `0x1F1`, the real-mode part in the low 32 KiB of the file, the 32-bit part
//! at `(setup_sects + 1) * 512` and, for a `LOAD_HIGH` kernel, at `100000h`.

use crate::pc_boot::{CONVENTIONAL_TOP, EXTENDED_RAM};

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

/// One file inside an ISO9660 image, located but not copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IsoFile {
    pub lba: u32,
    pub size: u32,
}

impl IsoFile {
    /// Copy the file out of the image.
    pub fn read(&self, iso: &[u8]) -> Option<Vec<u8>> {
        let start = self.lba as usize * 2048;
        let end = start.checked_add(self.size as usize)?;
        iso.get(start..end).map(|bytes| bytes.to_vec())
    }
}

/// Locate a file by path inside an ISO9660 image.
///
/// The walk follows the directory records from the primary volume descriptor's
/// root directory: each record carries its own length, so a record that does
/// not fit in the rest of a 2048-byte block is followed by a zero byte and the
/// block ends. Version suffixes (`;1`) are ignored and names compare without
/// case, because that is how the format actually stores them.
pub fn iso_file(iso: &[u8], path: &str) -> Option<IsoFile> {
    let mut root = None;
    for sector in 16..40u32 {
        let block = iso.get(sector as usize * 2048..)?;
        match block[0] {
            1 => {
                // Primary volume descriptor: the root directory record sits at
                // 0x9C and is itself a directory record.
                let record = block.get(0x9C..0x9C + 34)?;
                root = Some((
                    u32::from_le_bytes([record[2], record[3], record[4], record[5]]),
                    u32::from_le_bytes([record[10], record[11], record[12], record[13]]),
                ));
                break;
            }
            255 => break,
            _ => {}
        }
    }
    let (mut lba, mut size) = root?;
    let mut current = (lba, size);
    let parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_uppercase())
        .collect();
    for (index, wanted) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        lba = current.0;
        size = current.1;
        let mut found = None;
        for record in directory_records(iso, lba, size) {
            let entry = &record.0;
            let name = record.1;
            let normalised = name.split(';').next().unwrap_or(&name).to_string();
            if normalised != *wanted && normalised != format!("{wanted}.") {
                continue;
            }
            let extent = u32::from_le_bytes([entry[2], entry[3], entry[4], entry[5]]);
            let length = u32::from_le_bytes([entry[10], entry[11], entry[12], entry[13]]);
            // A directory name in the middle of the path must be a directory,
            // and the final component must not be one.
            let is_dir = entry[25] & 0x02 != 0;
            if last == is_dir {
                continue;
            }
            found = Some((extent, length));
            break;
        }
        current = found?;
    }
    Some(IsoFile {
        lba: current.0,
        size: current.1,
    })
}

/// Every record of a directory, with its decoded name.
fn directory_records<'a>(iso: &'a [u8], lba: u32, size: u32) -> Vec<(&'a [u8], String)> {
    let mut records = Vec::new();
    let start = lba as usize * 2048;
    let end = (start + size as usize).min(iso.len());
    let mut at = start;
    while at < end {
        let length = iso[at] as usize;
        if length == 0 {
            // No further record fits: advance to the next block boundary.
            at = (at / 2048 + 1) * 2048;
            continue;
        }
        if let Some(record) = iso.get(at..at + length) {
            if record.len() >= 34 {
                let name_len = record[32] as usize;
                if let Some(name) = record.get(33..33 + name_len) {
                    records.push((record, String::from_utf8_lossy(name).to_string()));
                }
            }
            at += length;
        } else {
            break;
        }
    }
    records
}

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
#[derive(Clone, Debug)]
pub struct BootParams {
    /// Kernel command line.
    pub cmdline: String,
    /// Already-loaded initramfs: `(address, size)`.
    pub initrd: Option<(u64, u64)>,
}

impl Default for BootParams {
    fn default() -> Self {
        Self {
            cmdline: String::new(),
            initrd: None,
        }
    }
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
    // The memory map the kernel's setup code forwards to the 32-bit kernel.
    // Same ranges the firmware reports through `INT 15h E820`.
    let entries: [(u64, u64, u32); 3] = [
        (0, CONVENTIONAL_TOP, 1),
        (CONVENTIONAL_TOP, 0x10_0000 - CONVENTIONAL_TOP, 2),
        (PAYLOAD_HIGH, EXTENDED_RAM, 1),
    ];
    setup[0x1E8] = entries.len() as u8;
    for (index, (base, length, kind)) in entries.iter().enumerate() {
        let at = 0x2D0 + index * 20;
        setup[at..at + 8].copy_from_slice(&base.to_le_bytes());
        setup[at + 8..at + 16].copy_from_slice(&length.to_le_bytes());
        setup[at + 16..at + 20].copy_from_slice(&kind.to_le_bytes());
    }
    // Screen: keep the 80x25 text mode the machine already has, and tell the
    // kernel it may use the VGA console.
    setup[0x1FA..0x1FC].copy_from_slice(&3u16.to_le_bytes());
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
