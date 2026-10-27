//! The multiboot 1 information structure (QEMU's `load_multiboot` role,
//! decode side).
//!
//! Clean-room from the multiboot 0.6.96 specification: the loader builds the
//! information structure and passes `EAX = 0x2BADB002`, `EBX` = its physical
//! address. The structure's flags select the fields the loader filled in;
//! this builder sets exactly the flags for the fields it writes and nothing
//! else.

/// The magic the loader passes in `EAX`.
pub const BOOTLOADER_MAGIC: u32 = 0x2BAD_B002;

/// Information-structure flags (multiboot spec §3.3).
pub mod info_flags {
    /// `mem_lower`/`mem_upper` are valid.
    pub const MEMORY: u32 = 1 << 0;
    /// `boot_device` is valid.
    pub const BOOTDEV: u32 = 1 << 1;
    /// `cmdline` is valid.
    pub const CMDLINE: u32 = 1 << 2;
    /// `mods_count`/`mods_addr` are valid.
    pub const MODS: u32 = 1 << 3;
    /// The symbol table (a.out) is valid.
    pub const AOUT_SYMS: u32 = 1 << 4;
    /// The symbol table (ELF sections) is valid.
    pub const ELF_SHDR: u32 = 1 << 5;
    /// The memory map is valid.
    pub const MMAP: u32 = 1 << 6;
    /// The drives table is valid.
    pub const DRIVES: u32 = 1 << 7;
    /// The ROM configuration table is valid.
    pub const CONFIG_TABLE: u32 = 1 << 8;
    /// The boot loader name is valid.
    pub const BOOT_LOADER_NAME: u32 = 1 << 9;
    /// The APM table is valid.
    pub const APM_TABLE: u32 = 1 << 10;
    /// Video mode information is valid.
    pub const VIDEO: u32 = 1 << 11;
}

/// Facts the loader gathered for the structure.
#[derive(Clone, Debug)]
pub struct MultibootInfo {
    /// Lower memory in KiB (below 1 MiB, top at the EBDA).
    pub mem_lower: u32,
    /// Upper memory in KiB (above 1 MiB).
    pub mem_upper: u32,
    /// BIOS drive the kernel was loaded from (`0x00` floppy, `0x80` disk).
    pub boot_device: u32,
    /// The kernel command line.
    pub cmdline: String,
    /// The boot loader's own name.
    pub bootloader_name: String,
    /// The E820-style memory map, `(base, length, type)` in map order.
    pub memory_map: Vec<(u64, u64, u32)>,
}

/// The placement of the structure and its variable-length companions: the
/// structure itself, then the command line, the loader name, then the memory
/// map entries — each 4-byte aligned, the way the spec's layout expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultibootPlacement {
    /// Physical address passed in `EBX`.
    pub address: u64,
    /// Command line address (0 = none).
    pub cmdline_address: u32,
    /// Boot loader name address (0 = none).
    pub name_address: u32,
    /// Memory map address (0 = none).
    pub mmap_address: u32,
    /// Size of the built structure.
    pub size: usize,
}

/// Build the information structure at `address`, returning the placement
/// and the whole byte image to store there (structure + companions, packed
/// contiguously in the order: command line, memory map, loader name).
///
/// `address` must be 4-byte aligned; the spec requires it and the builder
/// checks it.
pub fn build(info: &MultibootInfo, address: u64) -> Result<(MultibootPlacement, Vec<u8>), String> {
    if address & 0x3 != 0 {
        return Err(format!(
            "multiboot info at {address:#x} is not 4-byte aligned"
        ));
    }
    let mut flags = info_flags::MEMORY | info_flags::BOOTDEV;
    if !info.cmdline.is_empty() {
        flags |= info_flags::CMDLINE;
    }
    flags |= info_flags::BOOT_LOADER_NAME;
    if !info.memory_map.is_empty() {
        flags |= info_flags::MMAP;
    }

    // The fixed part is the spec's 26 words (flags at 0 .. vbe_interface_len
    // at 100); the fields this loader does not fill stay zero with their
    // flags clear.
    let fixed = 104;
    let aligned_len = |bytes: usize| (bytes + 3) & !3;
    let cmdline_len = if info.cmdline.is_empty() {
        0
    } else {
        aligned_len(info.cmdline.len() + 1)
    };
    // One map entry: 4-byte size + 8-byte base + 8-byte length + 4-byte type.
    let mmap_len = if info.memory_map.is_empty() {
        0
    } else {
        info.memory_map.len() * 24
    };
    let cmdline_address = if cmdline_len > 0 {
        (address + fixed as u64) as u32
    } else {
        0
    };
    let mmap_address = if mmap_len > 0 {
        (address + fixed as u64 + cmdline_len as u64) as u32
    } else {
        0
    };
    let name_address = (address + fixed as u64 + cmdline_len as u64 + mmap_len as u64) as u32;

    let mut image = Vec::with_capacity(fixed + cmdline_len + mmap_len + 64);
    let u32le = |image: &mut Vec<u8>, value: u32| image.extend_from_slice(&value.to_le_bytes());

    u32le(&mut image, flags);
    u32le(&mut image, info.mem_lower);
    u32le(&mut image, info.mem_upper);
    u32le(&mut image, info.boot_device);
    u32le(&mut image, cmdline_address);
    u32le(&mut image, 0); // mods_count
    u32le(&mut image, 0); // mods_addr
    u32le(&mut image, 0); // syms a.out tabsize
    u32le(&mut image, 0); // syms a.out strsize
    u32le(&mut image, 0); // syms addr
    u32le(&mut image, 0); // syms shdr_num
    u32le(&mut image, 0); // syms shdr_size
    u32le(&mut image, 0); // syms shdr_addr
    u32le(&mut image, mmap_len as u32); // mmap_length
    u32le(&mut image, mmap_address); // mmap_addr
    u32le(&mut image, 0); // drives_length (flag not set)
    u32le(&mut image, 0); // drives_addr (flag not set)
    u32le(&mut image, 0); // config_table (flag not set)
    u32le(&mut image, name_address); // boot_loader_name
    u32le(&mut image, 0); // apm_table (flag not set)
    u32le(&mut image, 0); // vbe_control_info (VIDEO not set)
    u32le(&mut image, 0); // vbe_mode_info
    u32le(&mut image, 0); // vbe_mode
    u32le(&mut image, 0); // vbe_interface_seg
    u32le(&mut image, 0); // vbe_interface_off
    u32le(&mut image, 0); // vbe_interface_len
    debug_assert_eq!(image.len(), fixed);

    // Companions, in address order: command line, memory map, loader name.
    if !info.cmdline.is_empty() {
        image.extend_from_slice(info.cmdline.as_bytes());
        image.push(0);
        while image.len() % 4 != 0 {
            image.push(0);
        }
    }
    for (base, length, kind) in &info.memory_map {
        let entry_len = 20u32; // base + length + type, without the size field
        u32le(&mut image, entry_len);
        image.extend_from_slice(&base.to_le_bytes());
        image.extend_from_slice(&length.to_le_bytes());
        u32le(&mut image, *kind);
    }
    image.extend_from_slice(info.bootloader_name.as_bytes());
    image.push(0);
    while image.len() % 4 != 0 {
        image.push(0);
    }

    Ok((
        MultibootPlacement {
            address,
            cmdline_address,
            name_address,
            mmap_address,
            size: image.len(),
        },
        image,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> MultibootInfo {
        MultibootInfo {
            mem_lower: 639,
            mem_upper: 63 * 1024,
            boot_device: 0x8000_00FF,
            cmdline: "console=ttyS0".into(),
            bootloader_name: "mosaic".into(),
            memory_map: vec![(0, 0x9FC00, 1), (0x100000, 63 * 1024 * 1024, 1)],
        }
    }

    #[test]
    fn the_structure_is_built_with_exact_flags() {
        let (placement, image) = build(&info(), 0x1000).unwrap();
        assert_eq!(placement.address, 0x1000);
        assert_eq!(image.len() % 4, 0);
        let word = |at: usize| {
            u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
        };
        assert_eq!(word(0) & info_flags::MEMORY, info_flags::MEMORY);
        assert_eq!(word(0) & info_flags::MMAP, info_flags::MMAP);
        assert_eq!(word(0) & info_flags::VIDEO, 0);
        assert_eq!(word(4), 639);
        assert_eq!(word(8), 63 * 1024);
        // mmap_length (offset 52) counts 24 bytes per entry.
        assert_eq!(word(52), 48);
        assert_eq!(word(56), placement.mmap_address);
        // The command line sits at the placement's cmdline address.
        let cmdline_at = (placement.cmdline_address - 0x1000) as usize;
        let until_nul = image[cmdline_at..]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap();
        assert_eq!(&image[cmdline_at..cmdline_at + until_nul], b"console=ttyS0");
        // The memory map sits between the command line and the loader name.
        let mmap_at = (placement.mmap_address - 0x1000) as usize;
        let word_at = |at: usize| {
            u32::from_le_bytes([image[at], image[at + 1], image[at + 2], image[at + 3]])
        };
        assert_eq!(word_at(mmap_at), 20);
        let base = u64::from_le_bytes(image[mmap_at + 4..mmap_at + 12].try_into().unwrap());
        assert_eq!(base, 0);
        // The loader name lands at the placement's name address.
        let name_at = (placement.name_address - 0x1000) as usize;
        let until_nul = image[name_at..].iter().position(|byte| *byte == 0).unwrap();
        assert_eq!(&image[name_at..name_at + until_nul], b"mosaic");
    }

    #[test]
    fn an_unaligned_placement_is_rejected() {
        assert!(build(&info(), 0x1002).is_err());
    }
}
