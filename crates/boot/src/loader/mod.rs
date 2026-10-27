//! Kernel image container formats (QEMU `hw/core/loader.c` role).
//!
//! [`load`] auto-detects the container the way QEMU's loader does — ELF,
//! uImage, multiboot, PE/COFF, SREC/IHEX, bzImage, raw — and returns one
//! [`LoadedImage`]: the loadable segments plus the entry point. The
//! per-format modules export the same structure directly for callers that
//! already know what they hold.

pub mod elf;
pub mod multiboot;
pub mod pe;
pub mod srec;
pub mod uimage;

/// One loadable segment: copy `data` to `physical`, entry-relative info kept
/// aside. Segments are sorted and non-overlapping (a format that cannot
/// promise that is rejected).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub physical: u64,
    pub data: Vec<u8>,
    /// Whether the segment is executable (PT_LOAD with PF_X, PE .text, ...).
    pub executable: bool,
    /// Whether the segment is writable (PT_LOAD with PF_W, PE .data, ...).
    pub writable: bool,
}

/// A decoded kernel image, container-format facts only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedImage {
    /// The format that was recognized.
    pub kind: ImageKind,
    /// First instruction address, when the format carries one.
    pub entry: Option<u64>,
    /// Loadable segments in address order.
    pub segments: Vec<Segment>,
    /// Human-readable provenance (e.g. the uImage name or ELF machine).
    pub summary: String,
}

/// The container formats [`load`] recognizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ImageKind {
    Elf32,
    Elf64,
    LegacyUImage,
    MultiUImage,
    Pe32,
    Pe32Plus,
    Srec,
    IntelHex,
    Multiboot,
    /// A Linux x86 `bzImage` (header at `0x1F1`, handled by the boot
    /// protocol in `proto::x86_linux`, not by segment loading).
    BzImage,
    Raw,
}

/// Why an image could not be decoded.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LoaderError {
    /// The image is shorter than the format's fixed header.
    TooShort { format: &'static str, need: usize },
    /// A magic number did not match.
    BadMagic { format: &'static str, offset: usize },
    /// A length field ran past the end of the image.
    Truncated {
        format: &'static str,
        detail: String,
    },
    /// The format is recognized but this variant is unsupported.
    Unsupported {
        format: &'static str,
        detail: String,
    },
}

impl std::fmt::Display for LoaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { format, need } => {
                write!(f, "{format}: image shorter than its {need}-byte header")
            }
            Self::BadMagic { format, offset } => {
                write!(f, "{format}: bad magic at offset {offset:#x}")
            }
            Self::Truncated { format, detail } => write!(f, "{format}: truncated: {detail}"),
            Self::Unsupported { format, detail } => write!(f, "{format}: unsupported: {detail}"),
        }
    }
}

impl std::error::Error for LoaderError {}

/// Detect and decode a kernel image by its leading bytes.
///
/// The detection order is the formats' magic disjointness, cheapest first:
/// ELF (`7F 45 4C 46`), uImage (`27 05 19 56` big-endian), PE (`MZ` with a
/// valid PE header), SREC (`S`), Intel HEX (`:`), multiboot (the 0x1BADB002
/// header inside the first 8 KiB), bzImage (`HdrS` at 0x202) — and raw
/// binary as the fallback, exactly as QEMU's `load_image` treats an
/// unrecognized image.
pub fn load(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    if image.len() >= 4 && &image[..4] == b"\x7FELF" {
        return elf::load_elf(image);
    }
    if image.len() >= 4 && image[..4] == [0x27, 0x05, 0x19, 0x56] {
        return uimage::load_uimage(image);
    }
    if image.len() >= 2 && &image[..2] == b"MZ" {
        return pe::load_pe(image);
    }
    if let Some(first) = image.first() {
        if *first == b'S' {
            return srec::load_srec(image);
        }
        if *first == b':' {
            return srec::load_ihex(image);
        }
    }
    if let Some(loaded) = multiboot::find_header(image) {
        return Ok(loaded);
    }
    if let Some(loaded) = bz_image_looks_like(image) {
        return Ok(loaded);
    }
    Ok(LoadedImage {
        kind: ImageKind::Raw,
        entry: None,
        segments: vec![Segment {
            physical: 0,
            data: image.to_vec(),
            executable: true,
            writable: true,
        }],
        summary: format!("{} bytes raw", image.len()),
    })
}

/// A `bzImage` carries its header at 0x1F1, not at offset zero; segment
/// placement is the boot protocol's job (`proto::x86_linux`), so recognition
/// only reports the fact.
fn bz_image_looks_like(image: &[u8]) -> Option<LoadedImage> {
    if image.len() < 0x206 {
        return None;
    }
    let magic = u32::from_le_bytes([image[0x202], image[0x203], image[0x204], image[0x205]]);
    if magic != 0x5372_6448 {
        return None;
    }
    let protocol = u16::from_le_bytes([image[0x206], image[0x207]]);
    Some(LoadedImage {
        kind: ImageKind::BzImage,
        entry: None,
        segments: Vec::new(),
        summary: format!("bzImage, boot protocol {protocol:#06x}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_raw_image_falls_through_with_one_segment() {
        let image = vec![0x13, 0x37, 0x00];
        let loaded = load(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::Raw);
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].data, image);
        assert_eq!(loaded.entry, None);
    }

    #[test]
    fn detection_is_by_magic_not_extension() {
        // A 4-byte ELF magic with nothing behind it is still an ELF attempt.
        let image = vec![0x7F, b'E', b'L', b'F'];
        assert!(matches!(
            load(&image),
            Err(LoaderError::TooShort { format: "ELF", .. })
        ));
    }
}
