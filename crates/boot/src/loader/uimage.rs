//! Legacy and multi-file uImage loader (QEMU `load_uimage` role).
//!
//! Clean-room from the U-Boot `image.h` header layout: the 64-byte big-endian
//! header, magic `0x27051956`, CRC fields, and the two shapes a `-kernel`
//! operand can be: one image (`IH_TYPE_KERNEL`/`IH_TYPE_SCRIPT`/...) or a
//! multi-file image (`IH_TYPE_MULTI`: the data starts with a table of
//! `(size)` words terminated by zero, each followed by its payload).

use super::{ImageKind, LoadedImage, LoaderError, Segment};

const HEADER_SIZE: usize = 64;
const MAGIC: u32 = 0x2705_1956;

/// OS-arch identification header codes a uImage carries (subset).
pub mod ih {
    pub const OS_LINUX: u8 = 5;
    pub const OS_U_BOOT: u8 = 17;

    pub const ARCH_ARM: u8 = 2;
    pub const ARCH_I386: u8 = 1;
    pub const ARCH_MIPS: u8 = 5;
    pub const ARCH_PPC: u8 = 7;
    pub const ARCH_SH: u8 = 10;
    pub const ARCH_M68K: u8 = 4;
    pub const ARCH_NIOS2: u8 = 13;
    pub const ARCH_OPENRISC: u8 = 12;
    pub const ARCH_RISCV: u8 = 22;
    pub const ARCH_SANDBOX: u8 = 16;
    pub const ARCH_X86_64: u8 = 23;

    pub const TYPE_STANDALONE: u8 = 1;
    pub const TYPE_KERNEL: u8 = 2;
    pub const TYPE_RAMDISK: u8 = 3;
    pub const TYPE_MULTI: u8 = 4;
    pub const TYPE_FIRMWARE: u8 = 5;
    pub const TYPE_SCRIPT: u8 = 6;
    pub const TYPE_FLAT_DEVICE_TREE: u8 = 9;

    pub const COMP_NONE: u8 = 0;
    pub const COMP_GZIP: u8 = 1;
    pub const COMP_BZIP2: u8 = 2;
    pub const COMP_LZMA: u8 = 3;
    pub const COMP_LZO: u8 = 4;
}

/// The parsed 64-byte header, all multi-byte fields already big-endian.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UImageHeader {
    pub magic: u32,
    pub header_crc: u32,
    pub time: u32,
    pub data_size: u32,
    pub load: u32,
    pub entry: u32,
    pub data_crc: u32,
    pub os: u8,
    pub arch: u8,
    pub image_type: u8,
    pub compression: u8,
}

impl UImageHeader {
    pub fn name<'a>(&self, image: &'a [u8]) -> &'a str {
        std::str::from_utf8(&image[32..64]).unwrap_or("")
    }
}

/// Parse the 64-byte header of a legacy uImage.
pub fn parse_header(image: &[u8]) -> Result<UImageHeader, LoaderError> {
    if image.len() < HEADER_SIZE {
        return Err(LoaderError::TooShort {
            format: "uImage",
            need: HEADER_SIZE,
        });
    }
    let be32 = |offset: usize| {
        u32::from_be_bytes([
            image[offset],
            image[offset + 1],
            image[offset + 2],
            image[offset + 3],
        ])
    };
    let magic = be32(0);
    if magic != MAGIC {
        return Err(LoaderError::BadMagic {
            format: "uImage",
            offset: 0,
        });
    }
    Ok(UImageHeader {
        magic,
        header_crc: be32(4),
        time: be32(8),
        data_size: be32(12),
        load: be32(20),
        entry: be32(24),
        data_crc: be32(28),
        os: image[32],
        arch: image[33],
        image_type: image[34],
        compression: image[35],
    })
}

/// Load a legacy uImage. Compression other than `IH_COMP_NONE` is rejected
/// honestly — a decompressor is not a loader's job here, and QEMU likewise
/// refuses compressed uImages on `-kernel`.
pub fn load_uimage(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    let header = parse_header(image)?;
    if header.compression != ih::COMP_NONE {
        return Err(LoaderError::Unsupported {
            format: "uImage",
            detail: format!(
                "compression {} (only 0/none is loadable)",
                header.compression
            ),
        });
    }
    let data_start = HEADER_SIZE;
    let data_end = data_start
        .checked_add(header.data_size as usize)
        .ok_or_else(|| LoaderError::Truncated {
            format: "uImage",
            detail: "data size overflows".into(),
        })?;
    let data = image
        .get(data_start..data_end)
        .ok_or_else(|| LoaderError::Truncated {
            format: "uImage",
            detail: format!(
                "image is {} bytes, data needs {}..{}",
                image.len(),
                data_start,
                data_end
            ),
        })?;
    if header.image_type == ih::TYPE_MULTI {
        return load_multi(image, header, data);
    }
    Ok(LoadedImage {
        kind: ImageKind::LegacyUImage,
        entry: Some(u64::from(header.entry)),
        segments: vec![Segment {
            physical: u64::from(header.load),
            data: data.to_vec(),
            executable: header.image_type != ih::TYPE_RAMDISK,
            writable: true,
        }],
        summary: format!(
            "uImage '{}' os {} arch {} type {} load {:#010x} entry {:#010x}",
            header.name(image),
            header.os,
            header.arch,
            header.image_type,
            header.load,
            header.entry
        ),
    })
}

/// A multi-file image: a size table, then each payload. The first entry's
/// load address is the header's `load`; later entries follow contiguously
/// (U-Boot's `bootm` places them; a loader that needs separate addresses
/// reads the size table through [`multi_entries`]).
fn load_multi(image: &[u8], header: UImageHeader, data: &[u8]) -> Result<LoadedImage, LoaderError> {
    let entries = multi_entries(data)?;
    let mut segments = Vec::new();
    let mut physical = u64::from(header.load);
    for (offset, size) in entries {
        let end = offset
            .checked_add(size)
            .ok_or_else(|| LoaderError::Truncated {
                format: "uImage",
                detail: "multi entry overflows".into(),
            })?;
        let bytes = data
            .get(offset..end)
            .ok_or_else(|| LoaderError::Truncated {
                format: "uImage",
                detail: format!("multi entry {offset}..{end} past data end {}", data.len()),
            })?;
        segments.push(Segment {
            physical,
            data: bytes.to_vec(),
            executable: true,
            writable: true,
        });
        physical = physical.saturating_add(size as u64);
        // Align the next component the way U-Boot's bootm lays out images.
        physical = (physical + 3) & !3;
    }
    let components = segments.len();
    Ok(LoadedImage {
        kind: ImageKind::MultiUImage,
        entry: Some(u64::from(header.entry)),
        segments,
        summary: format!(
            "multi uImage '{}' with {components} components",
            header.name(image)
        ),
    })
}

/// The `(offset, size)` table of a multi-file image's data region.
pub fn multi_entries(data: &[u8]) -> Result<Vec<(usize, usize)>, LoaderError> {
    let mut sizes = Vec::new();
    let mut at = 0usize;
    loop {
        if at + 4 > data.len() {
            return Err(LoaderError::Truncated {
                format: "uImage",
                detail: "multi size table unterminated".into(),
            });
        }
        let size =
            u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        at += 4;
        if size == 0 {
            break;
        }
        sizes.push(size);
    }
    let mut entries = Vec::new();
    let mut offset = at;
    for size in sizes {
        entries.push((offset, size));
        offset += size;
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-assembled legacy uImage header + payload (a *format fixture*).
    fn uimage(load: u32, entry: u32, data: &[u8], image_type: u8) -> Vec<u8> {
        let mut image = vec![0u8; 64];
        image[0..4].copy_from_slice(&MAGIC.to_be_bytes());
        image[12..16].copy_from_slice(&(data.len() as u32).to_be_bytes());
        image[20..24].copy_from_slice(&load.to_be_bytes());
        image[24..28].copy_from_slice(&entry.to_be_bytes());
        image[32] = ih::OS_LINUX;
        image[33] = ih::ARCH_ARM;
        image[34] = image_type;
        image[35] = ih::COMP_NONE;
        image[36..40].copy_from_slice(b"test");
        image.extend_from_slice(data);
        image
    }

    #[test]
    fn a_legacy_kernel_image_loads_at_its_declared_address() {
        let image = uimage(0x0000_8000, 0x0000_8000, b"kernel-code", ih::TYPE_KERNEL);
        let loaded = load_uimage(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::LegacyUImage);
        assert_eq!(loaded.entry, Some(0x8000));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0x8000);
        assert_eq!(loaded.segments[0].data, b"kernel-code");
    }

    #[test]
    fn a_compressed_uimage_is_rejected() {
        let mut image = uimage(0x8000, 0x8000, b"zzz", ih::TYPE_KERNEL);
        image[35] = ih::COMP_GZIP;
        assert!(matches!(
            load_uimage(&image),
            Err(LoaderError::Unsupported {
                format: "uImage",
                ..
            })
        ));
    }

    #[test]
    fn a_multi_image_exposes_each_component() {
        let payload = b"kernel-code";
        let ramdisk = b"initrd-data";
        let mut data = Vec::new();
        data.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        data.extend_from_slice(&(ramdisk.len() as u32).to_be_bytes());
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(payload);
        data.extend_from_slice(ramdisk);
        let image = uimage(0x8000, 0x8000, &data, ih::TYPE_MULTI);
        let loaded = load_uimage(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::MultiUImage);
        assert_eq!(loaded.segments.len(), 2);
        assert_eq!(loaded.segments[0].data, payload);
        assert_eq!(loaded.segments[1].data, ramdisk);
    }
}
