//! initramfs cpio reader — the "newc" format the Linux kernel unpacks, and
//! the "crc" variant (070702).
//!
//! Clean-room from the cpio `newc` layout: the 110-byte ASCII header, the
//! name (NUL-terminated, padded to 4), the data (padded to 4), and the
//! `TRAILER!!!` end marker. QEMU and the kernel both treat an initramfs as
//! exactly this stream; a front-end extracting `-initrd` members and a boot
//! protocol computing sizes both read it here.

/// Header length of a `newc` record, ASCII fields included.
pub const HEADER: usize = 110;

/// Why a cpio could not be walked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CpioError {
    NotCpio,
    Truncated { at: usize },
}

/// One archive member: `(name, byte range of the data, mode bits)`.
pub type Member = (String, (usize, usize), u32);

/// Walk every member of a `newc`/`crc` cpio archive.
///
/// The `TRAILER!!!` member ends the archive; concatenated archives (the
/// kernel appends a second cpio to a first) continue after it, so the walk
/// resumes until a real end or the bytes run out.
pub fn members(archive: &[u8]) -> Result<Vec<Member>, CpioError> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at < archive.len() {
        if at + HEADER > archive.len() {
            return Err(CpioError::Truncated { at });
        }
        let magic = &archive[at..at + 6];
        if magic != b"070701" && magic != b"070702" {
            return Err(CpioError::NotCpio);
        }
        let hex32 = |offset: usize| -> Result<u32, CpioError> {
            let start = at + 6 + offset * 8;
            let field = archive
                .get(start..start + 8)
                .ok_or(CpioError::Truncated { at: start })?;
            let text = std::str::from_utf8(field).map_err(|_| CpioError::NotCpio)?;
            u32::from_str_radix(text, 16).map_err(|_| CpioError::NotCpio)
        };
        let mode = hex32(1)?;
        let size = hex32(6)? as usize;
        let namesize = hex32(11)? as usize;
        let name_at = at + HEADER;
        let name_end = name_at
            .checked_add(namesize)
            .ok_or(CpioError::Truncated { at: name_at })?;
        let name_bytes = archive
            .get(name_at..name_end)
            .ok_or(CpioError::Truncated { at: name_at })?;
        let name = String::from_utf8_lossy(name_bytes)
            .trim_end_matches('\0')
            .to_string();
        let data_at = (name_end + 3) & !3;
        let data_end = data_at
            .checked_add(size)
            .ok_or(CpioError::Truncated { at: data_at })?;
        if data_end > archive.len() {
            return Err(CpioError::Truncated { at: data_at });
        }
        if name == "TRAILER!!!" {
            // Padded to the next header (or the end of a concatenated
            // archive: the kernel checks 4-byte padding then continues).
            at = (data_end + 3) & !3;
            continue;
        }
        out.push((name, (data_at, data_end), mode));
        at = (data_end + 3) & !3;
    }
    Ok(out)
}

/// Read one member's bytes by name (the last member with that name wins, the
/// way a later archive overwrites an earlier one at unpack time).
pub fn read_member<'a>(archive: &'a [u8], name: &str) -> Result<Option<&'a [u8]>, CpioError> {
    let mut found = None;
    for (member, range, _mode) in members(archive)? {
        if member == name {
            found = Some(range);
        }
    }
    Ok(found.map(|(start, end)| &archive[start..end]))
}

/// Total unpacked size of the archive (the kernel's page accounting walks
/// the same members).
pub fn unpacked_size(archive: &[u8]) -> Result<usize, CpioError> {
    Ok(members(archive)?
        .into_iter()
        .map(|(_, (start, end), _)| end - start)
        .sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode one newc member (a *format fixture* for the parser).
    fn member(name: &str, data: &[u8], mode: u32) -> Vec<u8> {
        let field = |value: u32| format!("{value:08X}");
        let mut record = Vec::new();
        record.extend_from_slice(b"070701");
        record.extend_from_slice(field(0).as_bytes()); // ino
        record.extend_from_slice(field(mode).as_bytes()); // mode
        record.extend_from_slice(field(0).as_bytes()); // uid
        record.extend_from_slice(field(0).as_bytes()); // gid
        record.extend_from_slice(field(1).as_bytes()); // nlink
        record.extend_from_slice(field(0).as_bytes()); // mtime
        record.extend_from_slice(field(data.len() as u32).as_bytes()); // size
        record.extend_from_slice(field(0).as_bytes()); // devmajor
        record.extend_from_slice(field(0).as_bytes()); // devminor
        record.extend_from_slice(field(0).as_bytes()); // rdevmajor
        record.extend_from_slice(field(0).as_bytes()); // rdevminor
        record.extend_from_slice(field(name.len() as u32 + 1).as_bytes()); // namesize
        record.extend_from_slice(field(0).as_bytes()); // check
        let mut name_bytes = name.as_bytes().to_vec();
        name_bytes.push(0);
        record.extend_from_slice(&name_bytes);
        while record.len() % 4 != 0 {
            record.push(0);
        }
        record.extend_from_slice(data);
        while record.len() % 4 != 0 {
            record.push(0);
        }
        record
    }

    fn archive() -> Vec<u8> {
        let mut archive = member("init", b"#!/bin/sh", 0o0100755);
        archive.extend_from_slice(&member("TRAILER!!!", b"", 0));
        archive
    }

    #[test]
    fn members_walk_in_order() {
        let archive = archive();
        let members = members(&archive).unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].0, "init");
        assert_eq!(&archive[members[0].1 .0..members[0].1 .1], b"#!/bin/sh");
        assert_eq!(members[0].2 & 0o170000, 0o100000); // regular file
    }

    #[test]
    fn a_member_is_read_by_name() {
        let archive = archive();
        assert_eq!(
            read_member(&archive, "init").unwrap(),
            Some(&b"#!/bin/sh"[..])
        );
        assert_eq!(read_member(&archive, "missing").unwrap(), None);
    }

    #[test]
    fn garbage_is_rejected() {
        // Short garbage is a truncation; wrong magic is a rejection. Both
        // must be errors, never a loaded member.
        assert!(matches!(
            members(b"not a cpio"),
            Err(CpioError::Truncated { .. } | CpioError::NotCpio)
        ));
        assert!(matches!(members(&[b'0'; 116]), Err(CpioError::NotCpio)));
    }

    #[test]
    fn concatenated_archives_are_walked() {
        let mut concatenated = archive();
        concatenated.extend_from_slice(&member("init", b"second", 0o100644));
        concatenated.extend_from_slice(&member("TRAILER!!!", b"", 0));
        // The last member with a name wins, the way unpack overwrites.
        assert_eq!(
            read_member(&concatenated, "init").unwrap(),
            Some(&b"second"[..])
        );
        assert_eq!(unpacked_size(&concatenated).unwrap(), 15);
    }
}
