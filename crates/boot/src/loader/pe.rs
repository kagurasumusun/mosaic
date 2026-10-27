//! PE/COFF loader — Windows and EFI payloads (QEMU loads EFI/PE via
//! `load_efi`/`x86_load_linux` helpers; a ReactOS/Windows kernel and an EFI
//! stub both arrive as PE).
//!
//! Clean-room from the Microsoft PE format: the DOS header's `MZ`, the
//! `e_lfanew` pointer, the COFF header, the optional header (PE32 and PE32+),
//! and the section table. Segments are the sections with a raw data
//! pointer, loaded at `ImageBase + VirtualAddress`.

use super::{ImageKind, LoadedImage, LoaderError, Segment};

const DOS_MAGIC: [u8; 2] = *b"MZ";
const PE_SIGNATURE: [u8; 4] = [b'P', b'E', 0, 0];

/// The optional-header magic values.
const PE32: u16 = 0x010B;
const PE32_PLUS: u16 = 0x020B;

const SECTION_SIZE: usize = 40;

/// Load a PE image, PE32 or PE32+.
pub fn load_pe(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    if image.len() < 64 || image[..2] != DOS_MAGIC {
        return Err(LoaderError::BadMagic {
            format: "PE",
            offset: 0,
        });
    }
    let lfanew = u32::from_le_bytes([image[0x3C], image[0x3D], image[0x3E], image[0x3F]]) as usize;
    if lfanew + 4 > image.len() || image[lfanew..lfanew + 4] != PE_SIGNATURE {
        return Err(LoaderError::BadMagic {
            format: "PE",
            offset: lfanew,
        });
    }
    let coff = lfanew + 4;
    if coff + 20 > image.len() {
        return Err(LoaderError::TooShort {
            format: "PE",
            need: coff + 20,
        });
    }
    let machine = u16::from_le_bytes([image[coff], image[coff + 1]]);
    let number_of_sections = u16::from_le_bytes([image[coff + 2], image[coff + 3]]) as usize;
    let optional_size = u16::from_le_bytes([image[coff + 16], image[coff + 17]]) as usize;
    let optional = coff + 20;
    if optional + optional_size > image.len() {
        return Err(LoaderError::Truncated {
            format: "PE",
            detail: format!(
                "optional header {optional}..{} past image end",
                optional + optional_size
            ),
        });
    }
    let magic = u16::from_le_bytes([image[optional], image[optional + 1]]);
    let (kind, is_plus) = match magic {
        PE32 => (ImageKind::Pe32, false),
        PE32_PLUS => (ImageKind::Pe32Plus, true),
        other => {
            return Err(LoaderError::Unsupported {
                format: "PE",
                detail: format!("optional header magic {other:#06x}"),
            })
        }
    };
    let entry_rva = u32::from_le_bytes([
        image[optional + 16],
        image[optional + 17],
        image[optional + 18],
        image[optional + 19],
    ]) as u64;
    let image_base = if is_plus {
        let mut base = [0u8; 8];
        base.copy_from_slice(&image[optional + 24..optional + 32]);
        u64::from_le_bytes(base)
    } else {
        u64::from(u32::from_le_bytes([
            image[optional + 28],
            image[optional + 29],
            image[optional + 30],
            image[optional + 31],
        ]))
    };
    let sections = optional + optional_size;
    let mut segments = Vec::new();
    for index in 0..number_of_sections {
        let at = sections + index * SECTION_SIZE;
        if at + SECTION_SIZE > image.len() {
            return Err(LoaderError::Truncated {
                format: "PE",
                detail: format!("section {index} header at {at:#x} past image end"),
            });
        }
        let section = &image[at..at + SECTION_SIZE];
        let virtual_size =
            u32::from_le_bytes([section[8], section[9], section[10], section[11]]) as u64;
        let virtual_address =
            u32::from_le_bytes([section[12], section[13], section[14], section[15]]) as u64;
        let raw_size =
            u32::from_le_bytes([section[16], section[17], section[18], section[19]]) as usize;
        let raw_pointer =
            u32::from_le_bytes([section[20], section[21], section[22], section[23]]) as usize;
        let characteristics =
            u32::from_le_bytes([section[36], section[37], section[38], section[39]]);
        if raw_size == 0 || raw_pointer == 0 {
            // BSS-like sections have no raw data; the loader's job ends at
            // the initialized image, as in a real loader's memory image.
            continue;
        }
        let _ = virtual_size;
        let end = raw_pointer
            .checked_add(raw_size)
            .ok_or_else(|| LoaderError::Truncated {
                format: "PE",
                detail: format!("section {index} raw range overflows"),
            })?;
        let data = image
            .get(raw_pointer..end)
            .ok_or_else(|| LoaderError::Truncated {
                format: "PE",
                detail: format!(
                    "section {index} raw {raw_pointer:#x}..{end:#x} past image end {:#x}",
                    image.len()
                ),
            })?
            .to_vec();
        segments.push(Segment {
            physical: image_base + virtual_address,
            data,
            executable: characteristics & 0x2000_0000 != 0, // IMAGE_SCN_MEM_EXECUTE
            writable: characteristics & 0x8000_0000 != 0,   // IMAGE_SCN_MEM_WRITE
        });
    }
    segments.sort_by_key(|segment| segment.physical);
    Ok(LoadedImage {
        kind,
        entry: Some(image_base + entry_rva),
        segments,
        summary: format!(
            "PE{} machine {machine:#06x}, {} sections, base {image_base:#x}",
            if is_plus { "+" } else { "" },
            number_of_sections
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-assembled minimal PE32: one executable section (a *format
    /// fixture* for the parser, not a guest image).
    fn pe32() -> Vec<u8> {
        // 0x200 bytes: the section table at 0x138 needs 40 bytes of room.
        let mut image = vec![0u8; 0x200];
        image[..2].copy_from_slice(&DOS_MAGIC);
        image[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes()); // e_lfanew
        let at = 0x40;
        image[at..at + 4].copy_from_slice(&PE_SIGNATURE);
        let coff = at + 4;
        image[coff..coff + 2].copy_from_slice(&0x014Cu16.to_le_bytes()); // i386
        image[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // sections
        image[coff + 16..coff + 18].copy_from_slice(&0xE0u16.to_le_bytes()); // optional size
        let optional = coff + 20;
        image[optional..optional + 2].copy_from_slice(&PE32.to_le_bytes());
        image[optional + 16..optional + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry RVA
        image[optional + 28..optional + 32].copy_from_slice(&0x0040_0000u32.to_le_bytes()); // base
        let section = optional + 0xE0;
        image[section..section + 8].copy_from_slice(b".text\0\0\0");
        image[section + 8..section + 12].copy_from_slice(&4u32.to_le_bytes()); // virtual size
        image[section + 12..section + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // RVA
        image[section + 16..section + 20].copy_from_slice(&4u32.to_le_bytes()); // raw size
        image[section + 20..section + 24].copy_from_slice(&0x80u32.to_le_bytes()); // raw ptr
        image[section + 36..section + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes()); // code, exec, read
        image[0x80..0x84].copy_from_slice(b"code");
        image
    }

    #[test]
    fn a_pe32_loads_its_section_at_the_image_base() {
        let image = pe32();
        let loaded = load_pe(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::Pe32);
        assert_eq!(loaded.entry, Some(0x0040_1000));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0x0040_1000);
        assert_eq!(loaded.segments[0].data, b"code");
        assert!(loaded.segments[0].executable);
        assert!(!loaded.segments[0].writable);
    }

    #[test]
    fn a_dos_image_without_a_pe_header_is_rejected() {
        // 0x200 bytes: the section table at 0x138 needs 40 bytes of room.
        let mut image = vec![0u8; 0x200];
        image[..2].copy_from_slice(&DOS_MAGIC);
        assert!(matches!(
            load_pe(&image),
            Err(LoaderError::BadMagic { format: "PE", .. })
        ));
    }
}
