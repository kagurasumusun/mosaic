//! ISO9660 volume parsing and the El Torito boot record — the image-format
//! half of booting from a CD, with no machine vocabulary in it.
//!
//! This is the QEMU `hw/core/loader.c` role: a loader utility every machine
//! shares, placed in modelkit so a front-end (resolving `image.iso!<path>`
//! operands), any CD-booting board and the boot tests all read images through
//! one parser. What lives here is clean-room format handling only: walking
//! directory records off the primary volume descriptor, and extracting the
//! El Torito boot image a firmware would load. What a machine *does* with the
//! bytes (drive numbering, load segments it chooses itself, BIOS services)
//! belongs to the machine model, not to the format.
//!
//! Clean-room from ISO 9660:1988 (directory records, volume descriptors) and
//! the El Torito Bootable CD Specification 1.0 (boot record volume descriptor
//! type 0, catalog validation and initial entries).

/// Logical block size of an ISO9660 volume; every LBA in the format counts
/// these, not 512-byte sectors.
pub const LOGICAL_BLOCK: usize = 2048;

/// Where an El Torito no-emulation boot image is conventionally loaded when
/// the catalog's initial entry names segment zero. The El Torito
/// specification's own default (`0x07C0`, the boot-sector convention).
pub const DEFAULT_LOAD_SEGMENT: u16 = 0x07C0;

/// One file inside an ISO9660 image, located but not copied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IsoFile {
    pub lba: u32,
    pub size: u32,
}

impl IsoFile {
    /// Copy the file out of the image.
    pub fn read(&self, iso: &[u8]) -> Option<Vec<u8>> {
        let start = self.lba as usize * LOGICAL_BLOCK;
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
        let block = iso.get(sector as usize * LOGICAL_BLOCK..)?;
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
fn directory_records(iso: &[u8], lba: u32, size: u32) -> Vec<(&[u8], String)> {
    let mut records = Vec::new();
    let start = lba as usize * LOGICAL_BLOCK;
    let end = (start + size as usize).min(iso.len());
    let mut at = start;
    while at < end {
        let length = iso[at] as usize;
        if length == 0 {
            // No further record fits: advance to the next block boundary.
            at = (at / LOGICAL_BLOCK + 1) * LOGICAL_BLOCK;
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

/// An extracted El Torito boot image: where it loads, how long it is, and the
/// bytes themselves. Pure format data — the machine that asked for it decides
/// what the numbers mean.
#[derive(Clone, Debug)]
pub struct ElToritoImage {
    pub load_segment: u16,
    pub sector_count: u16,
    pub load_lba: u32,
    pub media: u8,
    pub bytes: Vec<u8>,
}

/// Locate and extract the El Torito boot image of an ISO9660 image.
///
/// Clean-room from the El Torito 1.0 specification: the Boot Record Volume
/// Descriptor is the volume descriptor with type 0, and its bytes 0x47..0x4B
/// name the boot catalog's LBA. The catalog's initial entry describes the
/// default boot image.
pub fn el_torito_image(iso: &[u8]) -> Result<ElToritoImage, String> {
    // Volume descriptors start at LBA 16 and run until a terminator.
    let mut boot_record = None;
    for lba in 16..32usize {
        let at = lba * LOGICAL_BLOCK;
        if at + LOGICAL_BLOCK > iso.len() {
            break;
        }
        let descriptor = &iso[at..at + LOGICAL_BLOCK];
        if &descriptor[1..6] != b"CD001" {
            return Err(format!("volume descriptor {lba} is not CD001"));
        }
        match descriptor[0] {
            0 => {
                if &descriptor[7..30] == b"EL TORITO SPECIFICATION" {
                    boot_record = Some(descriptor);
                }
            }
            255 => break,
            _ => {}
        }
    }
    let Some(boot_record) = boot_record else {
        return Err("no El Torito boot record volume descriptor".into());
    };
    let catalog_lba = u32::from_le_bytes([
        boot_record[0x47],
        boot_record[0x48],
        boot_record[0x49],
        boot_record[0x4A],
    ]);
    let catalog_at = catalog_lba as usize * LOGICAL_BLOCK;
    if catalog_at + 64 > iso.len() {
        return Err("boot catalog lies outside the image".into());
    }
    let catalog = &iso[catalog_at..catalog_at + 64];
    // Validation entry: header id 0x01, a 16-bit word checksum over the first
    // 32 bytes that sums to zero, and the 0x55AA key. The 24-byte id string in
    // between is vendor text and is deliberately not interpreted.
    if catalog[0] != 0x01 {
        return Err("boot catalog validation entry has no header id".into());
    }
    let sum: u32 = catalog[..32]
        .chunks(2)
        .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
        .sum();
    if sum % 0x1_0000 != 0 {
        return Err("boot catalog validation entry checksum mismatch".into());
    }
    if catalog[30] != 0x55 || catalog[31] != 0xAA {
        return Err("boot catalog validation entry key is missing".into());
    }
    let entry = &catalog[32..64];
    if entry[0] != 0x88 {
        return Err("boot catalog initial entry is not bootable".into());
    }
    let media = entry[1] & 0x0F;
    let load_segment = u16::from_le_bytes([entry[2], entry[3]]);
    let sector_count = u16::from_le_bytes([entry[6], entry[7]]);
    let load_lba = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]);
    let start = load_lba as usize * LOGICAL_BLOCK;
    let length = usize::from(sector_count) * 512;
    if sector_count == 0 || start + length > iso.len() {
        return Err("boot image lies outside the image".into());
    }
    Ok(ElToritoImage {
        load_segment: if load_segment == 0 {
            DEFAULT_LOAD_SEGMENT
        } else {
            load_segment
        },
        sector_count,
        load_lba,
        media,
        bytes: iso[start..start + length].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the smallest in-memory ISO9660 volume that exercises the parsers:
    /// a primary volume descriptor whose root directory holds one file, plus
    /// an El Torito boot record and catalog. This is a *format* fixture for
    /// parser unit tests, not a guest image.
    fn minimal_iso() -> Vec<u8> {
        let mut iso = vec![0u8; 20 * LOGICAL_BLOCK];
        // Primary volume descriptor at LBA 16.
        let pvd = &mut iso[16 * LOGICAL_BLOCK..17 * LOGICAL_BLOCK];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        // Root directory record at 0x9C: extent LBA 3, length 2048.
        let root = &mut pvd[0x9C..0x9C + 34];
        root[0] = 34; // record length
        root[2..6].copy_from_slice(&3u32.to_le_bytes());
        root[10..14].copy_from_slice(&2048u32.to_le_bytes());
        root[25] = 0x02; // directory flag
        root[32] = 1; // name length
        root[33] = 0; // name: "\0" (root)
                      // Directory at LBA 3: a "." self-entry first, then one file record
                      // ("KERNEL" at LBA 5, 6 bytes) — the layout a real image walks.
        let dir = &mut iso[3 * LOGICAL_BLOCK..4 * LOGICAL_BLOCK];
        let mut dot = [0u8; 34];
        dot[0] = 34;
        dot[2..6].copy_from_slice(&3u32.to_le_bytes());
        dot[10..14].copy_from_slice(&2048u32.to_le_bytes());
        dot[25] = 0x02;
        dot[32] = 1;
        dot[33] = 0;
        dir[..34].copy_from_slice(&dot);
        let mut record = [0u8; 40];
        record[0] = 40;
        record[2..6].copy_from_slice(&5u32.to_le_bytes());
        record[10..14].copy_from_slice(&6u32.to_le_bytes());
        record[25] = 0;
        record[32] = 6;
        record[33..39].copy_from_slice(b"KERNEL");
        dir[34..74].copy_from_slice(&record);
        // File data at LBA 5.
        iso[5 * LOGICAL_BLOCK..5 * LOGICAL_BLOCK + 6].copy_from_slice(b"vmlinz");
        // Volume descriptor set terminator at LBA 18.
        let term = &mut iso[18 * LOGICAL_BLOCK..19 * LOGICAL_BLOCK];
        term[0] = 255;
        term[1..6].copy_from_slice(b"CD001");
        // Boot record volume descriptor at LBA 17, catalog at LBA 6.
        let br = &mut iso[17 * LOGICAL_BLOCK..18 * LOGICAL_BLOCK];
        br[0] = 0;
        br[1..6].copy_from_slice(b"CD001");
        br[7..30].copy_from_slice(b"EL TORITO SPECIFICATION");
        br[0x47..0x4B].copy_from_slice(&6u32.to_le_bytes());
        // Boot catalog at LBA 6: validation entry + initial entry.
        let catalog = &mut iso[6 * LOGICAL_BLOCK..6 * LOGICAL_BLOCK + 64];
        catalog[0] = 0x01;
        catalog[1..24].copy_from_slice(&[0x42; 23]); // vendor id, arbitrary
        catalog[30] = 0x55;
        catalog[31] = 0xAA;
        // Checksum: the 16-bit words of the first 32 bytes must sum to zero
        // modulo 0x10000.
        let sum: u32 = catalog[..32]
            .chunks(2)
            .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .sum();
        let checksum = (0x1_0000 - (sum % 0x1_0000)) as u16;
        catalog[28..30].copy_from_slice(&checksum.to_le_bytes());
        let entry = &mut catalog[32..64];
        entry[0] = 0x88; // bootable
        entry[1] = 0x04; // media type 4: no-emulation
        entry[2..4].copy_from_slice(&0u16.to_le_bytes()); // segment 0 -> default
        entry[6..8].copy_from_slice(&1u16.to_le_bytes()); // one "sector" (512 B)
        entry[8..12].copy_from_slice(&5u32.to_le_bytes()); // image at LBA 5
        iso
    }

    #[test]
    fn a_file_is_found_by_path_and_read_back() {
        let iso = minimal_iso();
        let file = iso_file(&iso, "/KERNEL").expect("file exists");
        assert_eq!(file.lba, 5);
        assert_eq!(file.size, 6);
        assert_eq!(file.read(&iso).as_deref(), Some(b"vmlinz".as_ref()));
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let iso = minimal_iso();
        assert!(iso_file(&iso, "/NOPE").is_none());
    }

    #[test]
    fn the_boot_catalog_yields_its_image() {
        let iso = minimal_iso();
        let image = el_torito_image(&iso).expect("bootable");
        assert_eq!(image.media, 4);
        assert_eq!(image.load_segment, DEFAULT_LOAD_SEGMENT);
        assert_eq!(image.bytes.len(), 512);
        assert_eq!(&image.bytes[..6], b"vmlinz");
    }

    #[test]
    fn a_volume_without_a_boot_record_is_rejected() {
        let mut iso = minimal_iso();
        // Turn the boot record into an ordinary terminator.
        let br = &mut iso[17 * LOGICAL_BLOCK..18 * LOGICAL_BLOCK];
        br[0] = 255;
        assert!(el_torito_image(&iso).is_err());
    }
}
