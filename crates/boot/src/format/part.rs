//! MBR and GPT partition tables (QEMU reads these wherever a boot order
//! walks a disk: `block.c`, `hw/block/*`).
//!
//! Clean-room from the classical MBR layout (partition entries at 0x1BE,
//! signature 55AA at 0x1FE) and the UEFI GPT layout (header at LBA 1, entry
//! array at the header's LBA, 128-byte entries). Extended partitions (MBR
//! types 0x05/0x0F/0x85) are walked to their logical volumes like every
//! bootloader does.

/// One partition entry, MBR or GPT, normalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Partition {
    /// Index in walk order (primary MBR slots first, then logical).
    pub index: usize,
    /// First sector (LBA).
    pub first_lba: u64,
    /// Sector count.
    pub sectors: u64,
    /// MBR partition type byte (0 for GPT entries).
    pub type_byte: u8,
    /// GPT type GUID (all zero for MBR entries).
    pub type_guid: [u8; 16],
    /// Whether the MBR boot flag is set / the GPT entry is used.
    pub bootable: bool,
}

/// MBR partition type bytes a boot loader distinguishes.
pub mod mbr_type {
    /// Empty entry.
    pub const EMPTY: u8 = 0x00;
    /// FAT12 (CHS).
    pub const FAT12: u8 = 0x01;
    /// FAT16 <32 MiB.
    pub const FAT16_SMALL: u8 = 0x04;
    /// Extended partition (CHS).
    pub const EXTENDED_CHS: u8 = 0x05;
    /// FAT16 >=32 MiB.
    pub const FAT16_LARGE: u8 = 0x06;
    /// HPFS/NTFS/exFAT.
    pub const NTFS: u8 = 0x07;
    /// FAT32 with LBA.
    pub const FAT32_LBA: u8 = 0x0C;
    /// Extended partition with LBA.
    pub const EXTENDED_LBA: u8 = 0x0F;
    /// Linux swap.
    pub const LINUX_SWAP: u8 = 0x82;
    /// Linux filesystem (any).
    pub const LINUX: u8 = 0x83;
}

/// GPT partition type GUIDs a boot loader distinguishes (UEFI 2.10 §5.3.3).
pub mod gpt_type {
    /// EFI System Partition.
    pub const EFI_SYSTEM: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    /// Linux filesystem data.
    pub const LINUX_FS: [u8; 16] = [
        0xAF, 0x3D, 0xC6, 0x0F, 0x83, 0x84, 0x72, 0x47, 0x8E, 0x79, 0x3D, 0x69, 0xD8, 0x47, 0x7D,
        0xE4,
    ];
    /// Microsoft basic data.
    pub const MS_DATA: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];
    /// BIOS boot partition.
    pub const BIOS_BOOT: [u8; 16] = [
        0x67, 0x8B, 0x27, 0x21, 0x61, 0xF4, 0x8D, 0x44, 0x84, 0x73, 0x2F, 0x00, 0x2B, 0xFC, 0x45,
        0xDA,
    ];
}

/// Parse an MBR's four primary entries. `None` when the signature is absent.
pub fn mbr_primary(disk: &[u8]) -> Option<Vec<Partition>> {
    if disk.len() < 512 {
        return None;
    }
    if disk[510] != 0x55 || disk[511] != 0xAA {
        return None;
    }
    let mut partitions = Vec::new();
    for slot in 0..4 {
        let entry = 0x1BE + slot * 16;
        let type_byte = disk[entry + 4];
        if type_byte == mbr_type::EMPTY {
            continue;
        }
        partitions.push(Partition {
            index: partitions.len(),
            first_lba: u32::from_le_bytes([
                disk[entry + 8],
                disk[entry + 9],
                disk[entry + 10],
                disk[entry + 11],
            ]) as u64,
            sectors: u32::from_le_bytes([
                disk[entry + 12],
                disk[entry + 13],
                disk[entry + 14],
                disk[entry + 15],
            ]) as u64,
            type_byte,
            type_guid: [0; 16],
            bootable: disk[entry] & 0x80 != 0,
        });
    }
    Some(partitions)
}

/// Walk the MBR: primary partitions, then every logical volume inside the
/// extended partition chain (EBRs), in on-disk order.
pub fn mbr_all(disk: &[u8]) -> Option<Vec<Partition>> {
    let mut partitions = mbr_primary(disk)?;
    // The extended chain starts at the first extended primary entry.
    let extended = partitions
        .iter()
        .find(|partition| {
            matches!(
                partition.type_byte,
                mbr_type::EXTENDED_CHS | mbr_type::EXTENDED_LBA | 0x85
            )
        })
        .map(|partition| partition.first_lba);
    if let Some(mut ebr_lba) = extended {
        let mut index = partitions.len();
        let mut visited = 0;
        while let Some(ebr) = disk.get(ebr_lba as usize * 512..(ebr_lba as usize + 1) * 512) {
            if visited > 128 || ebr[510] != 0x55 || ebr[511] != 0xAA {
                break;
            }
            let first = u32::from_le_bytes([
                ebr[0x1BE + 8],
                ebr[0x1BE + 9],
                ebr[0x1BE + 10],
                ebr[0x1BE + 11],
            ]) as u64;
            let sectors = u32::from_le_bytes([
                ebr[0x1BE + 12],
                ebr[0x1BE + 13],
                ebr[0x1BE + 14],
                ebr[0x1BE + 15],
            ]) as u64;
            let type_byte = ebr[0x1BE + 4];
            if type_byte != mbr_type::EMPTY && sectors > 0 {
                partitions.push(Partition {
                    index,
                    first_lba: ebr_lba + first,
                    sectors,
                    type_byte,
                    type_guid: [0; 16],
                    bootable: false,
                });
                index += 1;
            }
            let next = u32::from_le_bytes([
                ebr[0x1CE + 8],
                ebr[0x1CE + 9],
                ebr[0x1CE + 10],
                ebr[0x1CE + 11],
            ]);
            visited += 1;
            if next == 0 {
                break;
            }
            ebr_lba = {
                let base = extended.unwrap_or(0);
                base + next as u64
            };
        }
    }
    Some(partitions)
}

/// Parse a GPT: the entry array from LBA 1's header. `None` when the disk is
/// not GPT. The partition entry array's CRC is verified against the header.
pub fn gpt_entries(disk: &[u8]) -> Option<Vec<Partition>> {
    if disk.len() < 512 + 92 {
        return None;
    }
    let header = &disk[512..512 + 92];
    if header[..8] != *b"EFI PART" {
        return None;
    }
    let u32le = |at: usize| {
        u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let u64le = |at: usize| {
        let mut word = [0u8; 8];
        word.copy_from_slice(&header[at..at + 8]);
        u64::from_le_bytes(word)
    };
    let partition_entry_lba = u64le(72);
    let number_of_entries = u32le(80) as usize;
    let entry_size = u32le(84) as usize;
    let array_crc = u32le(88);
    if entry_size == 0 || number_of_entries == 0 || number_of_entries > 1024 {
        return None;
    }
    let array_start = partition_entry_lba as usize * 512;
    let array_len = number_of_entries * entry_size;
    let array = disk.get(array_start..array_start + array_len)?;
    // CRC32 (IEEE) of the entry array, the way the header records it.
    if crc32(array) != array_crc {
        return None;
    }
    let mut partitions = Vec::new();
    for index in 0..number_of_entries {
        let entry = &array[index * entry_size..(index + 1) * entry_size];
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&entry[..16]);
        if type_guid == [0u8; 16] {
            continue;
        }
        let u64le = |at: usize| {
            let mut word = [0u8; 8];
            word.copy_from_slice(&entry[at..at + 8]);
            u64::from_le_bytes(word)
        };
        partitions.push(Partition {
            index: partitions.len(),
            first_lba: u64le(32),
            sectors: u64le(40).saturating_sub(u64le(32)),
            type_byte: 0,
            type_guid,
            bootable: true,
        });
    }
    Some(partitions)
}

/// CRC-32 (IEEE 802.3, reflected, poly 0xEDB88320) — the GPT's checksum.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An MBR with one bootable FAT32-LBA partition (a *format fixture*).
    fn mbr_disk() -> Vec<u8> {
        let mut disk = vec![0u8; 512];
        disk[510] = 0x55;
        disk[511] = 0xAA;
        let entry = 0x1BE;
        disk[entry] = 0x80;
        disk[entry + 4] = mbr_type::FAT32_LBA;
        disk[entry + 8..entry + 12].copy_from_slice(&2048u32.to_le_bytes());
        disk[entry + 12..entry + 16].copy_from_slice(&4096u32.to_le_bytes());
        disk
    }

    #[test]
    fn the_primary_table_is_decoded() {
        let disk = mbr_disk();
        let partitions = mbr_primary(&disk).unwrap();
        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].first_lba, 2048);
        assert_eq!(partitions[0].sectors, 4096);
        assert!(partitions[0].bootable);
        assert_eq!(partitions[0].type_byte, mbr_type::FAT32_LBA);
    }

    #[test]
    fn a_disk_without_the_signature_has_no_table() {
        let disk = vec![0u8; 512];
        assert!(mbr_primary(&disk).is_none());
    }

    #[test]
    fn a_gpt_entry_array_with_a_valid_crc_decodes() {
        // One entry array at LBA 2, one used entry.
        let mut entries = vec![0u8; 128];
        entries[..16].copy_from_slice(&gpt_type::EFI_SYSTEM);
        entries[32..40].copy_from_slice(&2048u64.to_le_bytes());
        entries[40..48].copy_from_slice(&4096u64.to_le_bytes());
        let crc = crc32(&entries);
        let mut disk = vec![0u8; 512 * 3 + 128];
        disk[510] = 0x55;
        disk[511] = 0xAA;
        disk[512..520].copy_from_slice(b"EFI PART");
        disk[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        disk[512 + 80..512 + 84].copy_from_slice(&1u32.to_le_bytes());
        disk[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        disk[512 + 88..512 + 92].copy_from_slice(&crc.to_le_bytes());
        disk[512 * 2..512 * 2 + 128].copy_from_slice(&entries);
        let partitions = gpt_entries(&disk).unwrap();
        assert_eq!(partitions.len(), 1);
        assert_eq!(partitions[0].first_lba, 2048);
        assert_eq!(partitions[0].sectors, 2048);
        assert_eq!(partitions[0].type_guid, gpt_type::EFI_SYSTEM);
    }

    #[test]
    fn a_gpt_with_a_bad_array_crc_is_rejected() {
        let mut disk = vec![0u8; 512 * 3 + 128];
        disk[512..520].copy_from_slice(b"EFI PART");
        disk[512 + 72..512 + 80].copy_from_slice(&2u64.to_le_bytes());
        disk[512 + 80..512 + 84].copy_from_slice(&1u32.to_le_bytes());
        disk[512 + 84..512 + 88].copy_from_slice(&128u32.to_le_bytes());
        disk[512 + 88..512 + 92].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(gpt_entries(&disk).is_none());
    }
}
