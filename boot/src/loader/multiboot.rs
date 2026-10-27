//! Multiboot header finder (GRUB multiboot 1, `hw/core/loader.c`'s
//! `load_multiboot` role).
//!
//! The multiboot header is *inside* the first 8 KiB of the image, aligned to
//! 4 bytes, and begins with the magic `0x1BADB002`. This module finds it and
//! reports the facts the multiboot specification assigns to the header
//! (entry, load addresses, `bss_end`); the boot-protocol side (building the
//! information structure a multiboot kernel expects in `EAX/EBX`) lives in
//! `proto::multiboot`.

use super::{ImageKind, LoadedImage, LoaderError, Segment};

/// The multiboot 1 magic.
pub const MB_MAGIC: u32 = 0x1BAD_B002;
/// The header is inside this many bytes of the image start.
pub const HEADER_SEARCH: usize = 8192;

/// The multiboot header as the specification lays it out (flags at +4, then
/// the address fields the `AOUT_KLUDGE` flags enable).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultibootHeader {
    pub flags: u32,
    pub checksum: u32,
    pub header_addr: u32,
    pub load_addr: u32,
    pub load_end_addr: u32,
    pub bss_end_addr: u32,
    pub entry_addr: u32,
    /// Offset of the header inside the image.
    pub header_offset: usize,
}

/// Header flag bits (multiboot spec §3.1.1).
pub mod flags {
    /// Modules must be page aligned.
    pub const PAGE_ALIGN: u32 = 1 << 0;
    /// Memory information is requested.
    pub const MEMORY_INFO: u32 = 1 << 1;
    /// Video mode information is requested.
    pub const NEED_VIDEO_INFO: u32 = 1 << 2;
    /// The `AOUT_KLUDGE`: the header's address fields are valid.
    pub const AOUT_KLUDGE: u32 = 1 << 16;
}

impl MultibootHeader {
    /// Whether the header carries valid load addresses.
    pub fn has_load_addresses(self) -> bool {
        self.flags & flags::AOUT_KLUDGE != 0
    }
}

/// Search the first 8 KiB for a multiboot header. Returns the loader result
/// when found (segments from the header's address fields), `None` otherwise.
pub fn find_header(image: &[u8]) -> Option<LoadedImage> {
    let header = parse_header(image)?;
    let mut segments = Vec::new();
    if header.has_load_addresses() {
        // The kludge: [load_addr, load_end_addr) is the initialized image,
        // located at header_offset + (load_addr - header_addr) in the file.
        let file_offset = header.header_offset as u64 + u64::from(header.load_addr)
            - u64::from(header.header_addr);
        let size = if header.load_end_addr != 0 {
            u64::from(header.load_end_addr) - u64::from(header.load_addr)
        } else {
            image.len() as u64 - file_offset
        };
        let start = file_offset as usize;
        let end = start.checked_add(size as usize)?;
        let data = image.get(start..end)?.to_vec();
        if !data.is_empty() {
            segments.push(Segment {
                physical: u64::from(header.load_addr),
                data,
                executable: true,
                writable: true,
            });
        }
    }
    Some(LoadedImage {
        kind: ImageKind::Multiboot,
        entry: Some(u64::from(header.entry_addr)),
        segments,
        summary: format!(
            "multiboot, flags {:#010x}, entry {:#010x}",
            header.flags, header.entry_addr
        ),
    })
}

/// Parse the header, checking the checksum (`magic + flags + checksum == 0`).
pub fn parse_header(image: &[u8]) -> Option<MultibootHeader> {
    let search = image.len().min(HEADER_SEARCH);
    let mut offset = 0;
    while offset + 12 <= search {
        if offset % 4 != 0 {
            offset += 4 - (offset % 4);
            continue;
        }
        let magic = u32::from_le_bytes([
            image[offset],
            image[offset + 1],
            image[offset + 2],
            image[offset + 3],
        ]);
        if magic != MB_MAGIC {
            offset += 4;
            continue;
        }
        let word = |at: usize| -> u32 {
            u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
        };
        let flags = word(offset + 4);
        let checksum = word(offset + 8);
        if magic.wrapping_add(flags).wrapping_add(checksum) != 0 {
            offset += 4;
            continue;
        }
        let mut header = MultibootHeader {
            flags,
            checksum,
            header_addr: 0,
            load_addr: 0,
            load_end_addr: 0,
            bss_end_addr: 0,
            entry_addr: 0,
            header_offset: offset,
        };
        if offset + 28 <= search && flags & flags::AOUT_KLUDGE != 0 {
            header.header_addr = word(offset + 12);
            header.load_addr = word(offset + 16);
            header.load_end_addr = word(offset + 20);
            header.bss_end_addr = word(offset + 24);
            header.entry_addr = word(offset + 28);
        }
        return Some(header);
    }
    None
}

/// The error the proto side reports when a multiboot boot cannot be built.
pub fn expect_header() -> LoaderError {
    LoaderError::Unsupported {
        format: "multiboot",
        detail: "no multiboot header in the first 8 KiB".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal multiboot image with the AOUT_KLUDGE (a *format fixture*).
    fn multiboot_image() -> Vec<u8> {
        let entry = 0x0010_0100u32;
        let load = 0x0010_0000u32;
        let flags = flags::AOUT_KLUDGE | flags::MEMORY_INFO;
        let checksum = 0u32.wrapping_sub(MB_MAGIC).wrapping_sub(flags);
        let mut image = vec![0u8; 256];
        // Header at file offset 32 (searched in 4-byte steps); the payload's
        // file offset is header_offset + (load - header_addr) = 32 - 32 = 0.
        let at = 32;
        image[at..at + 4].copy_from_slice(&MB_MAGIC.to_le_bytes());
        image[at + 4..at + 8].copy_from_slice(&flags.to_le_bytes());
        image[at + 8..at + 12].copy_from_slice(&checksum.to_le_bytes());
        // header_addr: the physical address where the header itself sits.
        let header_addr = load + at as u32;
        image[at + 12..at + 16].copy_from_slice(&header_addr.to_le_bytes());
        image[at + 16..at + 20].copy_from_slice(&load.to_le_bytes());
        image[at + 20..at + 24].copy_from_slice(&(load + 8).to_le_bytes());
        image[at + 24..at + 28].copy_from_slice(&(load + 64).to_le_bytes());
        image[at + 28..at + 32].copy_from_slice(&entry.to_le_bytes());
        // Eight bytes of payload at load+0: file offset 0, clear of the header.
        image[0..8].copy_from_slice(b"mbkernel");
        image
    }

    #[test]
    fn the_header_is_found_and_decoded() {
        let image = multiboot_image();
        let header = parse_header(&image).expect("header");
        assert_eq!(header.header_offset, 32);
        assert!(header.has_load_addresses());
        assert_eq!(header.entry_addr, 0x0010_0100);
        let loaded = find_header(&image).expect("loaded");
        assert_eq!(loaded.kind, ImageKind::Multiboot);
        assert_eq!(loaded.entry, Some(0x0010_0100));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0x0010_0000);
        assert_eq!(&loaded.segments[0].data, b"mbkernel");
    }

    #[test]
    fn a_bad_checksum_is_skipped_not_loaded() {
        let mut image = multiboot_image();
        // Corrupt the header's checksum field (header at 32, checksum at +8).
        image[32 + 8] ^= 0xFF;
        assert!(parse_header(&image).is_none());
        assert!(find_header(&image).is_none());
    }
}
