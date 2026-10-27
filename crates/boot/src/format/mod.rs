//! Disk and filesystem image formats (QEMU's `block/` probing role for
//! guest images, plus the boot-sector filesystem facts a bootloader reads).

pub mod cpio;
pub mod fat;
pub mod iso9660;
pub mod part;

/// What a disk image looks like at its first sectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DiskKind {
    /// An ISO9660 volume (a `CD001` volume descriptor at LBA 16).
    Iso9660,
    /// A GPT-partitioned disk (`EFI PART` at LBA 1 sector 1).
    Gpt,
    /// An MBR-partitioned disk (signature `55 AA` at offset 510).
    Mbr,
    /// None of the above — a raw, unpartitioned image.
    Raw,
}

/// Probe a disk image's layout by its leading sectors.
pub fn probe(disk: &[u8]) -> DiskKind {
    if disk.len() >= iso9660::LOGICAL_BLOCK * 17 {
        let at = 16 * iso9660::LOGICAL_BLOCK;
        if disk[at + 1..at + 6] == *b"CD001" {
            return DiskKind::Iso9660;
        }
    }
    if disk.len() >= 512 + 8 && disk[512..520] == *b"EFI PART" {
        return DiskKind::Gpt;
    }
    if disk.len() >= 512 && disk[510] == 0x55 && disk[511] == 0xAA {
        return DiskKind::Mbr;
    }
    DiskKind::Raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_image_is_raw() {
        assert_eq!(probe(&[]), DiskKind::Raw);
    }

    #[test]
    fn an_mbr_signature_is_detected() {
        let mut disk = vec![0u8; 512];
        disk[510] = 0x55;
        disk[511] = 0xAA;
        assert_eq!(probe(&disk), DiskKind::Mbr);
    }

    #[test]
    fn a_gpt_signature_is_detected() {
        let mut disk = vec![0u8; 1024];
        disk[512..520].copy_from_slice(b"EFI PART");
        assert_eq!(probe(&disk), DiskKind::Gpt);
    }
}
