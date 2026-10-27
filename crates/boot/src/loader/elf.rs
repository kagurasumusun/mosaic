//! ELF32/ELF64 loader (both endians), QEMU `load_elf` role.
//!
//! Clean-room from the System V ABI: the 16-byte identity, the program
//! headers, and `PT_LOAD` segments only. The entry point is `e_entry`.
//! Physical addresses are `p_paddr` when the ELF carries them (QEMU prefers
//! them for `-kernel`), `p_vaddr` otherwise.

use super::{ImageKind, LoadedImage, LoaderError, Segment};

const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ELFDATA2MSB: u8 = 2;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

/// Load an ELF image, 32- or 64-bit, little- or big-endian.
pub fn load_elf(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    if image.len() < 52 {
        return Err(LoaderError::TooShort {
            format: "ELF",
            need: 52,
        });
    }
    if &image[..4] != b"\x7FELF" {
        return Err(LoaderError::BadMagic {
            format: "ELF",
            offset: 0,
        });
    }
    let class = image[4];
    let data = image[5];
    let (kind, is_64, big_endian) = match (class, data) {
        (ELFCLASS32, ELFDATA2LSB) => (ImageKind::Elf32, false, false),
        (ELFCLASS32, ELFDATA2MSB) => (ImageKind::Elf32, false, true),
        (ELFCLASS64, ELFDATA2LSB) => (ImageKind::Elf64, true, false),
        (ELFCLASS64, ELFDATA2MSB) => (ImageKind::Elf64, true, true),
        _ => {
            return Err(LoaderError::Unsupported {
                format: "ELF",
                detail: format!("class {class}, encoding {data}"),
            })
        }
    };
    let read_u16 = |offset: usize| -> Result<u16, LoaderError> {
        let at = offset..offset + 2;
        let bytes = image.get(at).ok_or_else(|| truncated(offset))?;
        Ok(if big_endian {
            u16::from_be_bytes([bytes[0], bytes[1]])
        } else {
            u16::from_le_bytes([bytes[0], bytes[1]])
        })
    };
    let read_u32 = |offset: usize| -> Result<u32, LoaderError> {
        let at = offset..offset + 4;
        let bytes = image.get(at).ok_or_else(|| truncated(offset))?;
        Ok(if big_endian {
            u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        })
    };
    let read_u64 = |offset: usize| -> Result<u64, LoaderError> {
        let at = offset..offset + 8;
        let bytes = image.get(at).ok_or_else(|| truncated(offset))?;
        Ok(if big_endian {
            let mut w = [0u8; 8];
            w.copy_from_slice(bytes);
            u64::from_be_bytes(w)
        } else {
            let mut w = [0u8; 8];
            w.copy_from_slice(bytes);
            u64::from_le_bytes(w)
        })
    };
    let entry = if is_64 {
        read_u64(24)?
    } else {
        u64::from(read_u32(24)?)
    };
    let phoff = if is_64 {
        read_u64(32)?
    } else {
        u64::from(read_u32(28)?)
    };
    let phentsize = usize::from(read_u16(if is_64 { 54 } else { 42 })?);
    let phnum = usize::from(read_u16(if is_64 { 56 } else { 44 })?);
    let machine = read_u16(18)?;

    let mut segments = Vec::new();
    for index in 0..phnum {
        let base = phoff as usize + index * phentsize;
        let (p_type, offset, vaddr, paddr, filesz, flags) = if is_64 {
            (
                read_u32(base)?,
                read_u64(base + 8)?,
                read_u64(base + 16)?,
                read_u64(base + 24)?,
                read_u64(base + 32)?,
                read_u32(base + 4)?,
            )
        } else {
            (
                read_u32(base)?,
                u64::from(read_u32(base + 4)?),
                u64::from(read_u32(base + 8)?),
                u64::from(read_u32(base + 12)?),
                u64::from(read_u32(base + 16)?),
                read_u32(base + 24)?,
            )
        };
        if p_type != PT_LOAD {
            continue;
        }
        let start = offset as usize;
        let end = start
            .checked_add(filesz as usize)
            .ok_or_else(|| truncated(base))?;
        let data = image
            .get(start..end)
            .ok_or_else(|| truncated(start))?
            .to_vec();
        segments.push(Segment {
            physical: if paddr != 0 { paddr } else { vaddr },
            data,
            executable: flags & PF_X != 0,
            writable: flags & PF_W != 0,
        });
    }
    segments.sort_by_key(|segment| segment.physical);
    for pair in segments.windows(2) {
        let previous_end = pair[0]
            .physical
            .checked_add(pair[0].data.len() as u64)
            .ok_or_else(|| truncated(phoff as usize))?;
        if pair[1].physical < previous_end {
            return Err(LoaderError::Unsupported {
                format: "ELF",
                detail: "overlapping PT_LOAD segments".into(),
            });
        }
    }
    if segments.is_empty() {
        return Err(LoaderError::Unsupported {
            format: "ELF",
            detail: "no PT_LOAD segments".into(),
        });
    }
    Ok(LoadedImage {
        kind,
        entry: Some(entry),
        segments,
        summary: format!(
            "ELF{} {}-endian, machine {machine}, {} PT_LOAD segments",
            if is_64 { 64 } else { 32 },
            if big_endian { "big" } else { "little" },
            phnum
        ),
    })
}

fn truncated(offset: usize) -> LoaderError {
    LoaderError::Truncated {
        format: "ELF",
        detail: format!("header/segment past end of image near offset {offset:#x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-assembled minimal ELF32 little-endian image: one program
    /// header, one PT_LOAD of four bytes. A *format fixture* for the parser,
    /// not a guest image.
    fn elf32(data: &[u8], entry: u32, vaddr: u32) -> Vec<u8> {
        let mut image = vec![0u8; 52];
        image[..4].copy_from_slice(b"\x7FELF");
        image[4] = ELFCLASS32;
        image[5] = ELFDATA2LSB;
        image[6] = 1; // version
        image[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
        image[18..20].copy_from_slice(&3u16.to_le_bytes()); // EM_I386
        image[24..28].copy_from_slice(&entry.to_le_bytes());
        image[28..32].copy_from_slice(&52u32.to_le_bytes()); // e_phoff
        image[42..44].copy_from_slice(&32u16.to_le_bytes()); // e_phentsize
        image[44..46].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let mut phdr = vec![0u8; 32];
        phdr[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
        // p_offset: the payload sits after the 52-byte header + 32-byte phdr.
        phdr[4..8].copy_from_slice(&84u32.to_le_bytes());
        phdr[8..12].copy_from_slice(&vaddr.to_le_bytes()); // p_vaddr
        phdr[12..16].copy_from_slice(&vaddr.to_le_bytes()); // p_paddr
        phdr[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes()); // p_filesz
        phdr[20..24].copy_from_slice(&(data.len() as u32).to_le_bytes()); // p_memsz
        phdr[24..28].copy_from_slice(&(PF_X | PF_W).to_le_bytes()); // p_flags
        phdr[28..32].copy_from_slice(&0x1000u32.to_le_bytes()); // p_align
        image.extend_from_slice(&phdr);
        image.extend_from_slice(data);
        image
    }

    #[test]
    fn an_elf32_loads_its_single_segment_and_entry() {
        let image = elf32(b"code", 0x1000_1000, 0x0010_0000);
        let loaded = load_elf(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::Elf32);
        assert_eq!(loaded.entry, Some(0x1000_1000));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0x0010_0000);
        assert_eq!(loaded.segments[0].data, b"code");
        assert!(loaded.segments[0].executable);
        assert!(loaded.segments[0].writable);
    }

    #[test]
    fn the_loader_rejects_an_elf_without_pt_load() {
        let mut image = elf32(b"code", 0x1000, 0x1000);
        // The program header sits right after the 52-byte ELF header; turn
        // its single segment into PT_NULL.
        let at = 52;
        image[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            load_elf(&image),
            Err(LoaderError::Unsupported { format: "ELF", .. })
        ));
    }

    #[test]
    fn bad_magic_is_rejected_by_name() {
        let image = vec![0u8; 64];
        assert_eq!(
            load_elf(&image),
            Err(LoaderError::BadMagic {
                format: "ELF",
                offset: 0
            })
        );
    }
}
