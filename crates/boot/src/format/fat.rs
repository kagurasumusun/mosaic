//! FAT12/16/32 reader (read-only), the format half of booting from a floppy
//! or a DOS/Windows partition (QEMU's `-fda`/`-hda` boot sectors).
//!
//! Clean-room from Microsoft's FAT specification: the BPB (bios parameter
//! block), the FAT type decision (cluster count thresholds), the cluster
//! chains, and the 8.3 directory entries with LFN (long file name)
//! reconstruction. Writing FAT is out of scope — a boot loader only reads.

/// Sector size the FAT format assumes (BPB's `bytes_per_sector` must be this
/// or a power of two up to 4096; boot sectors are 512).
pub const SECTOR: usize = 512;

/// Why a FAT access failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FatError {
    NotFat,
    BadBpb { detail: &'static str },
    NotFound,
    NotAFile,
    Truncated,
}

/// The FAT type a volume's cluster count decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FatKind {
    Fat12,
    Fat16,
    Fat32,
}

/// A parsed FAT volume, located but not copied: every access re-reads the
/// disk slice, exactly the way a boot loader walks media it cannot hold.
pub struct FatVolume<'a> {
    disk: &'a [u8],
    pub kind: FatKind,
    /// First sector of the volume (the partition's start).
    pub start_sector: u64,
    /// Sectors per cluster (a power of two).
    pub sectors_per_cluster: u32,
    /// First data sector (relative to the volume start).
    pub first_data_sector: u32,
    /// Cluster count.
    pub cluster_count: u32,
    /// Root directory: FAT12/16 region sector, FAT32 cluster.
    pub root: RootLocation,
    /// Number of FATs and sectors per FAT.
    pub fat_copies: u32,
    pub sectors_per_fat: u32,
    /// First sector of the first FAT (relative to the volume start).
    pub fat_start: u32,
    /// Total sectors of the volume.
    pub total_sectors: u64,
    /// The volume label from the root directory's label entry, when present.
    pub label: Option<String>,
}

/// Where the root directory lives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootLocation {
    /// FAT12/16: a fixed region after the FATs.
    SectorRegion { start: u32, entries: u32 },
    /// FAT32: the first cluster of the root directory chain.
    Cluster(u32),
}

/// One located file: start cluster and byte length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FatFile {
    pub first_cluster: u32,
    pub size: u32,
}

impl<'a> FatVolume<'a> {
    /// Parse the volume starting at `start_sector` (a partition's first
    /// sector, or zero for a whole floppy image).
    pub fn parse(disk: &'a [u8], start_sector: u64) -> Result<Self, FatError> {
        let base = start_sector as usize * SECTOR;
        let boot = disk.get(base..base + SECTOR).ok_or(FatError::Truncated)?;
        let u16le = |at: usize| u16::from_le_bytes([boot[at], boot[at + 1]]);
        let u32le =
            |at: usize| u32::from_le_bytes([boot[at], boot[at + 1], boot[at + 2], boot[at + 3]]);
        let bytes_per_sector = u16le(11) as u32;
        if bytes_per_sector.count_ones() != 1 || !(512..=4096).contains(&bytes_per_sector) {
            return Err(FatError::BadBpb {
                detail: "bytes per sector is not a supported power of two",
            });
        }
        let sectors_per_cluster = boot[13] as u32;
        if sectors_per_cluster == 0 || sectors_per_cluster.count_ones() != 1 {
            return Err(FatError::BadBpb {
                detail: "sectors per cluster is not a power of two",
            });
        }
        let reserved = u16le(14) as u32;
        if reserved == 0 {
            return Err(FatError::BadBpb {
                detail: "reserved sectors is zero",
            });
        }
        let fat_copies = boot[16] as u32;
        if fat_copies == 0 {
            return Err(FatError::BadBpb {
                detail: "FAT copies is zero",
            });
        }
        let root_entries = u16le(17) as u32;
        let total16 = u16le(19) as u64;
        let media = boot[21];
        if media < 0xE5 && media.count_ones() != 1 {
            // Media descriptors are either >= 0xE5 or one-bit codes.
            return Err(FatError::BadBpb {
                detail: "media descriptor is invalid",
            });
        }
        let sectors_per_fat16 = u16le(22) as u32;
        let total32 = u32le(32) as u64;
        let total_sectors = if total16 != 0 { total16 } else { total32 };
        if total_sectors == 0 {
            return Err(FatError::BadBpb {
                detail: "total sectors is zero",
            });
        }
        let sectors_per_fat = if sectors_per_fat16 != 0 {
            sectors_per_fat16
        } else {
            u32le(36)
        };
        if sectors_per_fat == 0 {
            return Err(FatError::BadBpb {
                detail: "sectors per FAT is zero",
            });
        }
        let fat_start = reserved;
        let root_sectors = (root_entries * 32).div_ceil(bytes_per_sector);
        let first_data_sector = reserved + fat_copies * sectors_per_fat + root_sectors;
        let data_sectors = total_sectors.saturating_sub(first_data_sector as u64);
        let cluster_count = data_sectors / sectors_per_cluster as u64;
        let kind = if cluster_count < 4085 {
            FatKind::Fat12
        } else if cluster_count < 65_525 {
            FatKind::Fat16
        } else {
            FatKind::Fat32
        };
        let root = if kind == FatKind::Fat32 {
            let cluster = u32le(44);
            if cluster < 2 {
                return Err(FatError::BadBpb {
                    detail: "FAT32 root cluster is invalid",
                });
            }
            RootLocation::Cluster(cluster)
        } else {
            RootLocation::SectorRegion {
                start: first_data_sector - root_sectors,
                entries: root_entries,
            }
        };
        Ok(Self {
            disk,
            kind,
            start_sector,
            sectors_per_cluster,
            first_data_sector,
            cluster_count: cluster_count as u32,
            root,
            fat_copies,
            sectors_per_fat,
            fat_start,
            total_sectors,
            label: None,
        })
    }

    fn sector(&self, relative: u32) -> Option<&'a [u8]> {
        let at = (self.start_sector as usize + relative as usize) * SECTOR;
        self.disk.get(at..at + SECTOR)
    }

    /// The FAT entry for `cluster` (the next cluster, or an end marker).
    fn fat_entry(&self, cluster: u32) -> Option<u32> {
        let (offset, size) = match self.kind {
            FatKind::Fat12 => ((cluster as usize * 3) / 2, 2),
            FatKind::Fat16 => (cluster as usize * 2, 2),
            FatKind::Fat32 => (cluster as usize * 4, 4),
        };
        let fat = self.sector(self.fat_start)?;
        let end = offset + size;
        if end > fat.len() {
            return None;
        }
        let bytes = &fat[offset..end];
        Some(match self.kind {
            FatKind::Fat12 => {
                let value = u16::from_le_bytes([bytes[0], bytes[1]]);
                let entry = if cluster & 1 == 0 {
                    value & 0x0FFF
                } else {
                    value >> 4
                };
                u32::from(entry)
            }
            FatKind::Fat16 => u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            FatKind::Fat32 => {
                u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) & 0x0FFF_FFFF
            }
        })
    }

    fn is_end_of_chain(&self, entry: u32) -> bool {
        match self.kind {
            FatKind::Fat12 => entry >= 0x0FF8,
            FatKind::Fat16 => entry >= 0xFFF8,
            FatKind::Fat32 => entry >= 0x0FFF_FFF8,
        }
    }

    fn is_bad(&self, entry: u32) -> bool {
        match self.kind {
            FatKind::Fat12 => (0x0FF7..0x0FF8).contains(&entry),
            FatKind::Fat16 => (0xFFF7..0xFFF8).contains(&entry),
            FatKind::Fat32 => (0x0FFF_FFF7..0x0FFF_FFF8).contains(&entry),
        }
    }

    /// The byte range of one cluster on the disk.
    fn cluster_bytes(&self, cluster: u32) -> Option<&'a [u8]> {
        if cluster < 2 {
            return None;
        }
        let relative = self.first_data_sector + (cluster - 2) * self.sectors_per_cluster;
        let at = (self.start_sector as usize + relative as usize) * SECTOR;
        let len = self.sectors_per_cluster as usize * SECTOR;
        self.disk.get(at..at + len)
    }

    /// Walk a cluster chain, visiting each cluster's bytes.
    fn walk(&self, first: u32, mut visit: impl FnMut(&[u8]) -> bool) -> Option<()> {
        let mut cluster = first;
        let mut hops = 0;
        loop {
            hops += 1;
            if hops > self.cluster_count + 2 {
                return None;
            }
            let bytes = self.cluster_bytes(cluster)?;
            if !visit(bytes) {
                return Some(());
            }
            let next = self.fat_entry(cluster)?;
            if self.is_bad(next) || next == 0 {
                return None;
            }
            if self.is_end_of_chain(next) {
                return Some(());
            }
            cluster = next;
        }
    }

    /// Read a whole file by its located [`FatFile`].
    pub fn read_file(&self, file: &FatFile) -> Option<Vec<u8>> {
        let mut data = Vec::with_capacity(file.size as usize);
        let mut remaining = file.size as usize;
        self.walk(file.first_cluster, |bytes| {
            let take = remaining.min(bytes.len());
            data.extend_from_slice(&bytes[..take]);
            remaining -= take;
            remaining > 0
        })?;
        if data.len() != file.size as usize {
            // The chain ended short: a corrupted filesystem, not a guess.
            return None;
        }
        Some(data)
    }

    /// Locate a file by `/`-separated path from the root (8.3 names, or LFN;
    /// case-insensitive both ways).
    pub fn file(&self, path: &str) -> Result<FatFile, FatError> {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let mut entries = match self.root {
            RootLocation::Cluster(cluster) => self.read_dir_cluster(cluster),
            RootLocation::SectorRegion { start, entries } => self.read_dir_region(start, entries),
        }
        .ok_or(FatError::Truncated)?;
        for (depth, wanted) in parts.iter().enumerate() {
            let last = depth + 1 == parts.len();
            let found = entries
                .iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(wanted) && !entry.is_volume_label())
                .ok_or(FatError::NotFound)?;
            if last {
                if found.is_directory() {
                    return Err(FatError::NotAFile);
                }
                return Ok(FatFile {
                    first_cluster: found.first_cluster(),
                    size: found.size,
                });
            }
            if !found.is_directory() {
                return Err(FatError::NotFound);
            }
            entries = self
                .read_dir_cluster(found.first_cluster())
                .ok_or(FatError::Truncated)?;
        }
        Err(FatError::NotFound)
    }

    fn read_dir_cluster(&self, cluster: u32) -> Option<Vec<DirEntry>> {
        let mut raw = Vec::new();
        self.walk(cluster, |bytes| {
            raw.extend_from_slice(bytes);
            true
        })?;
        Some(parse_dir(&raw))
    }

    fn read_dir_region(&self, start: u32, entries: u32) -> Option<Vec<DirEntry>> {
        let bytes = entries as usize * 32;
        let at = (self.start_sector as usize + start as usize) * SECTOR;
        let raw = self.disk.get(at..at + bytes)?;
        Some(parse_dir(raw))
    }
}

/// One 8.3 directory entry (32 bytes) as parsed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntry {
    pub name: String,
    pub attributes: u8,
    /// First cluster (for FAT32, read both the low and high words).
    pub cluster_low: u16,
    pub cluster_high: u16,
    pub size: u32,
}

impl DirEntry {
    pub fn first_cluster(&self) -> u32 {
        u32::from(self.cluster_low) | (u32::from(self.cluster_high) << 16)
    }

    pub fn is_directory(&self) -> bool {
        self.attributes & 0x10 != 0
    }

    pub fn is_volume_label(&self) -> bool {
        self.attributes & 0x08 != 0 && self.attributes & 0x3F == 0x08
    }

    pub fn is_lfn(&self) -> bool {
        self.attributes == 0x0F
    }
}

/// Parse a raw directory region: 8.3 entries, with LFN sequences folded in.
fn parse_dir(raw: &[u8]) -> Vec<DirEntry> {
    let mut entries = Vec::new();
    let mut lfn: Vec<(u8, String)> = Vec::new();
    for chunk in raw.chunks(32) {
        if chunk.len() < 32 {
            break;
        }
        if chunk[0] == 0x00 {
            break; // No further entries.
        }
        if chunk[0] == 0xE5 {
            lfn.clear();
            continue; // Deleted.
        }
        let attributes = chunk[11];
        if attributes == 0x0F {
            // Long file name: sequence number, UTF-16 characters.
            let sequence = chunk[0] & 0x3F;
            let mut text = String::new();
            for (start, count) in [(1, 10usize), (14, 12), (28, 4)] {
                for index in 0..count / 2 {
                    let at = start + index * 2;
                    let unit = u16::from_le_bytes([chunk[at], chunk[at + 1]]);
                    if unit == 0x0000 || unit == 0xFFFF {
                        continue;
                    }
                    text.push(char::from_u32(u32::from(unit)).unwrap_or('?'));
                }
            }
            lfn.push((sequence, text));
            continue;
        }
        // An 8.3 entry: the short name, spaced by the format's rules.
        let mut name = String::new();
        let base = &chunk[..8];
        let extension = &chunk[8..11];
        name.push_str(String::from_utf8_lossy(base).trim_end());
        let extension = String::from_utf8_lossy(extension).trim_end().to_string();
        if !extension.is_empty() {
            name.push('.');
            name.push_str(&extension);
        }
        let mut final_name = name;
        if !lfn.is_empty() {
            lfn.sort_by_key(|(sequence, _)| *sequence);
            let joined: String = lfn
                .iter()
                .map(|(_, text)| text.as_str())
                .collect::<Vec<_>>()
                .join("");
            if !joined.is_empty() {
                final_name = joined;
            }
        }
        lfn.clear();
        entries.push(DirEntry {
            name: final_name,
            attributes,
            cluster_low: u16::from_le_bytes([chunk[26], chunk[27]]),
            cluster_high: u16::from_le_bytes([chunk[20], chunk[21]]),
            size: u32::from_le_bytes([chunk[28], chunk[29], chunk[30], chunk[31]]),
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small FAT12 floppy image: one reserved sector (the boot
    /// sector), two 1-sector FATs, a 16-entry root directory, and two data
    /// clusters. A *format fixture* for the parser.
    fn fat12_floppy() -> Vec<u8> {
        let bytes_per_sector = 512u32;
        let sectors_per_cluster = 1u32;
        let reserved = 1u32;
        let fat_copies = 2u32;
        let sectors_per_fat = 1u32;
        let root_entries = 16u32;
        let total = 32u64;
        let first_data = reserved + fat_copies * sectors_per_fat + 1; // root = 1 sector
        let cluster_count = (total - first_data as u64) / sectors_per_cluster as u64;

        let mut disk = vec![0u8; total as usize * SECTOR];
        let boot = &mut disk[..SECTOR];
        boot[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]); // jump
        boot[3..11].copy_from_slice(b"MOSAICFS");
        boot[11..13].copy_from_slice(&(bytes_per_sector as u16).to_le_bytes());
        boot[13] = sectors_per_cluster as u8;
        boot[14..16].copy_from_slice(&(reserved as u16).to_le_bytes());
        boot[16] = fat_copies as u8;
        boot[17..19].copy_from_slice(&(root_entries as u16).to_le_bytes());
        boot[19..21].copy_from_slice(&(total as u16).to_le_bytes());
        boot[21] = 0xF0; // media
        boot[22..24].copy_from_slice(&(sectors_per_fat as u16).to_le_bytes());
        boot[510] = 0x55;
        boot[511] = 0xAA;
        // FAT: entry 0/1 media markers, cluster 2 -> EOF (0xFFF), cluster 3
        // -> EOF (0xFF8). FAT12 packs two entries in three bytes: [3]=FF,
        // [4]=8F, [5]=FF encodes 0xFFF for cluster 2 and 0xFF8 for cluster 3.
        let fat_at = reserved as usize * SECTOR;
        for copy in 0..fat_copies {
            let at = fat_at + copy as usize * sectors_per_fat as usize * SECTOR;
            disk[at] = 0xF0;
            disk[at + 1] = 0xFF;
            disk[at + 2] = 0xFF;
            disk[at + 3] = 0xFF;
            disk[at + 4] = 0x8F;
            disk[at + 5] = 0xFF;
        }
        // Root directory: one file "KERNEL.BIN" at cluster 2, 6 bytes.
        let root_at = (first_data - 1) as usize * SECTOR;
        let entry = &mut disk[root_at..root_at + 32];
        entry[..8].copy_from_slice(b"KERNEL  ");
        entry[8..11].copy_from_slice(b"BIN");
        entry[11] = 0x20; // archive
        entry[26..28].copy_from_slice(&2u16.to_le_bytes());
        entry[28..32].copy_from_slice(&6u32.to_le_bytes());
        // One subdirectory "DIR" at cluster 3 with one file inside.
        let dir_entry = &mut disk[root_at + 32..root_at + 64];
        dir_entry[..8].copy_from_slice(b"DIR     ");
        dir_entry[8..11].copy_from_slice(b"   ");
        dir_entry[11] = 0x10;
        dir_entry[26..28].copy_from_slice(&3u16.to_le_bytes());
        // Cluster 3 holds DIR's entries: "." and ".." then "README.TXT".
        let cluster3 = (first_data as usize + 1) * SECTOR;
        let dot = &mut disk[cluster3..cluster3 + 32];
        dot[..8].copy_from_slice(b".       ");
        dot[11] = 0x10;
        dot[26..28].copy_from_slice(&3u16.to_le_bytes());
        let dotdot = &mut disk[cluster3 + 32..cluster3 + 64];
        dotdot[..8].copy_from_slice(b"..      ");
        dotdot[11] = 0x10;
        let readme = &mut disk[cluster3 + 64..cluster3 + 96];
        readme[..8].copy_from_slice(b"README  ");
        readme[8..11].copy_from_slice(b"TXT");
        readme[11] = 0x20;
        readme[26..28].copy_from_slice(&2u16.to_le_bytes());
        readme[28..32].copy_from_slice(&5u32.to_le_bytes());
        // Data: cluster 2 content (KERNEL.BIN and README.TXT share it).
        let cluster2 = first_data as usize * SECTOR;
        disk[cluster2..cluster2 + 6].copy_from_slice(b"vmlinu");
        assert_eq!(cluster_count, 32 - 4);
        disk
    }

    #[test]
    fn a_fat12_volume_parses_its_bpb() {
        let disk = fat12_floppy();
        let volume = FatVolume::parse(&disk, 0).unwrap();
        assert_eq!(volume.kind, FatKind::Fat12);
        assert_eq!(volume.sectors_per_cluster, 1);
        assert_eq!(volume.cluster_count, 28);
        assert!(matches!(
            volume.root,
            RootLocation::SectorRegion { entries: 16, .. }
        ));
    }

    #[test]
    fn a_file_is_located_and_read_back() {
        let disk = fat12_floppy();
        let volume = FatVolume::parse(&disk, 0).unwrap();
        let file = volume.file("/KERNEL.BIN").unwrap();
        assert_eq!(file.first_cluster, 2);
        assert_eq!(volume.read_file(&file).as_deref(), Some(b"vmlinu".as_ref()));
    }

    #[test]
    fn paths_walk_directories() {
        let disk = fat12_floppy();
        let volume = FatVolume::parse(&disk, 0).unwrap();
        let file = volume.file("/DIR/README.TXT").unwrap();
        assert_eq!(file.first_cluster, 2);
        assert_eq!(file.size, 5);
    }

    #[test]
    fn a_directory_path_is_not_a_file() {
        let disk = fat12_floppy();
        let volume = FatVolume::parse(&disk, 0).unwrap();
        assert_eq!(volume.file("/DIR"), Err(FatError::NotAFile));
        assert_eq!(volume.file("/MISSING"), Err(FatError::NotFound));
    }
}
