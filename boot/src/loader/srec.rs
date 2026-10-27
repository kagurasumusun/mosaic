//! Motorola SREC and Intel HEX loaders — the firmware-ROM formats.
//!
//! SREC (S-record) and Intel HEX carry (address, bytes) pairs in ASCII, the
//! way firmware ROM images ship. Both loaders verify each record's checksum
//! the way the formats define them (SREC: the byte sum of count, address,
//! data and checksum is `0xFF`; IHEX: the byte sum of every record byte
//! including the checksum is zero), then produce one [`LoadedImage`] whose
//! segments are the address-sorted, coalesced data runs. The entry point
//! comes from SREC's S7/S8/S9 termination record when present.

use super::{ImageKind, LoadedImage, LoaderError, Segment};

/// The most data bytes one S-record may carry (count field is one byte and
/// covers address, data and checksum).
const SREC_MAX_DATA: usize = 250;

/// Load an SREC image (S1/S2/S3 data, S0 comment, S5/S6 counts, S7/S8/S9
/// start addresses).
pub fn load_srec(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    let mut runs: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut entry = None;
    for (index, line) in lines(image) {
        let record = parse_srec(line).ok_or_else(|| bad("SREC", index, "not an S-record"))?;
        match record {
            SrecRecord::Header(_) => {}
            SrecRecord::Data { address, data } => runs.push((address, data)),
            SrecRecord::Count(_) => {}
            SrecRecord::Start { address } => entry = Some(address),
        }
    }
    if runs.is_empty() {
        return Err(LoaderError::Unsupported {
            format: "SREC",
            detail: "no data records".into(),
        });
    }
    Ok(LoadedImage {
        kind: ImageKind::Srec,
        entry,
        segments: coalesce(runs, "SREC")?,
        summary: "SREC firmware image".into(),
    })
}

#[allow(dead_code)] // Header/Count payloads are parse facts, kept for reporting
enum SrecRecord {
    Header(String),
    Data { address: u64, data: Vec<u8> },
    Count(u16),
    Start { address: u64 },
}

fn parse_srec(line: &str) -> Option<SrecRecord> {
    let line = line.trim();
    let bytes = line.as_bytes();
    if bytes.len() < 8 || bytes[0] != b'S' {
        return None;
    }
    let kind = match bytes[1] {
        byte @ b'0'..=b'9' => byte,
        _ => return None,
    };
    let payload = &bytes[2..];
    if payload.len() % 2 != 0 {
        return None;
    }
    let mut decoded = Vec::with_capacity(payload.len() / 2);
    for pair in 0..payload.len() / 2 {
        let high = hex_value(payload[pair * 2])?;
        let low = hex_value(payload[pair * 2 + 1])?;
        decoded.push((high << 4) | low);
    }
    // decoded = [count, address..., data..., checksum]; count covers
    // address + data + checksum.
    let count = decoded[0] as usize;
    if count < 2 || count + 1 > decoded.len() {
        return None;
    }
    let sum: u8 = decoded[..=count]
        .iter()
        .fold(0u8, |accumulator, byte| accumulator.wrapping_add(*byte));
    if sum != 0xFF {
        return None;
    }
    let body = &decoded[1..count]; // address + data, checksum dropped
    match kind {
        b'0' => Some(SrecRecord::Header(
            String::from_utf8_lossy(body).to_string(),
        )),
        b'1' => data_record(body, 2),
        b'2' => data_record(body, 3),
        b'3' => data_record(body, 4),
        b'5' | b'6' => {
            let value = u16::from_be_bytes([body[0], body[1]]);
            Some(SrecRecord::Count(value))
        }
        b'7' => start_record(body, 4),
        b'8' => start_record(body, 3),
        b'9' => start_record(body, 2),
        _ => None,
    }
}

fn data_record(body: &[u8], address_size: usize) -> Option<SrecRecord> {
    if body.len() < address_size || body.len() > address_size + SREC_MAX_DATA {
        return None;
    }
    let mut address = 0u64;
    for byte in &body[..address_size] {
        address = (address << 8) | u64::from(*byte);
    }
    Some(SrecRecord::Data {
        address,
        data: body[address_size..].to_vec(),
    })
}

fn start_record(body: &[u8], address_size: usize) -> Option<SrecRecord> {
    if body.len() < address_size {
        return None;
    }
    let mut address = 0u64;
    for byte in &body[..address_size] {
        address = (address << 8) | u64::from(*byte);
    }
    Some(SrecRecord::Start { address })
}

/// Load an Intel HEX image (types 00 data, 01 EOF, 02 extended segment,
/// 04 extended linear address).
pub fn load_ihex(image: &[u8]) -> Result<LoadedImage, LoaderError> {
    let mut runs: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut extended: u64 = 0;
    for (index, line) in lines(image) {
        let bytes = line.trim().as_bytes();
        if bytes.is_empty() {
            continue;
        }
        if bytes.first() != Some(&b':') {
            return Err(bad("IHEX", index, "no leading ':'"));
        }
        let hex = &bytes[1..];
        if hex.len() % 2 != 0 {
            return Err(bad("IHEX", index, "odd hex digit count"));
        }
        let mut record = Vec::with_capacity(hex.len() / 2);
        for pair in 0..hex.len() / 2 {
            let high =
                hex_value(hex[pair * 2]).ok_or_else(|| bad("IHEX", index, "bad hex digit"))?;
            let low =
                hex_value(hex[pair * 2 + 1]).ok_or_else(|| bad("IHEX", index, "bad hex digit"))?;
            record.push((high << 4) | low);
        }
        if record.len() < 5 {
            return Err(bad("IHEX", index, "record shorter than its fixed fields"));
        }
        // Checksum: the byte sum of the whole record (including it) is zero.
        let sum: u8 = record
            .iter()
            .fold(0u8, |accumulator, byte| accumulator.wrapping_add(*byte));
        if sum != 0 {
            return Err(bad("IHEX", index, "checksum mismatch"));
        }
        let length = record[0] as usize;
        let offset = u16::from_be_bytes([record[1], record[2]]);
        let record_type = record[3];
        let data = record
            .get(4..4 + length)
            .ok_or_else(|| bad("IHEX", index, "data shorter than length byte"))?
            .to_vec();
        match record_type {
            0x00 => runs.push((extended | u64::from(offset), data)),
            0x01 => break,
            0x02 => {
                if data.len() != 2 {
                    return Err(bad("IHEX", index, "extended segment not 2 bytes"));
                }
                extended = u64::from(u16::from_be_bytes([data[0], data[1]])) << 4;
            }
            0x04 => {
                if data.len() != 2 {
                    return Err(bad("IHEX", index, "extended linear not 2 bytes"));
                }
                extended = u64::from(u16::from_be_bytes([data[0], data[1]])) << 16;
            }
            other => {
                return Err(LoaderError::Unsupported {
                    format: "IHEX",
                    detail: format!("line {}: record type {other:#04x}", index + 1),
                })
            }
        }
    }
    if runs.is_empty() {
        return Err(LoaderError::Unsupported {
            format: "IHEX",
            detail: "no data records".into(),
        });
    }
    Ok(LoadedImage {
        kind: ImageKind::IntelHex,
        entry: None,
        segments: coalesce(runs, "IHEX")?,
        summary: "Intel HEX firmware image".into(),
    })
}

/// Coalesce (address, data) runs into sorted, gap-merged segments.
fn coalesce(
    mut runs: Vec<(u64, Vec<u8>)>,
    format: &'static str,
) -> Result<Vec<Segment>, LoaderError> {
    runs.sort_by_key(|(address, _)| *address);
    let mut segments: Vec<Segment> = Vec::new();
    for (address, data) in runs {
        match segments.last_mut() {
            Some(last) if last.physical + last.data.len() as u64 == address => {
                last.data.extend_from_slice(&data);
            }
            Some(last) if last.physical + last.data.len() as u64 > address => {
                return Err(LoaderError::Unsupported {
                    format,
                    detail: "overlapping data records".into(),
                });
            }
            _ => segments.push(Segment {
                physical: address,
                data,
                executable: true,
                writable: true,
            }),
        }
    }
    Ok(segments)
}

fn bad(format: &'static str, index: usize, why: &str) -> LoaderError {
    LoaderError::Unsupported {
        format,
        detail: format!("line {}: {why}", index + 1),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn lines(image: &[u8]) -> impl Iterator<Item = (usize, &str)> {
    std::str::from_utf8(image)
        .unwrap_or("")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode one SREC data record: type, address, data, with the count and
    /// checksum the format requires. A *format fixture* for the parser.
    fn srec_line(kind: u8, address: u16, data: &[u8]) -> String {
        let mut record = vec![0u8];
        record.extend_from_slice(&address.to_be_bytes());
        record.extend_from_slice(data);
        // count covers address + data + checksum, i.e. every byte after itself.
        record[0] = record.len() as u8;
        let checksum =
            0xFFu8.wrapping_sub(record.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
        record.push(checksum);
        let mut line = String::new();
        line.push('S');
        line.push(kind as char);
        for byte in &record {
            line.push_str(&format!("{byte:02X}"));
        }
        line
    }

    fn srec_image() -> Vec<u8> {
        let mut text = srec_line(b'1', 0x0000, &[0xDE, 0xAD, 0xBE, 0xEF]);
        text.push('\n');
        text.push_str(&srec_line(b'1', 0x0004, &[0xCA, 0xFE, 0xBA, 0xBE]));
        text.push('\n');
        text.push_str(&srec_line(b'9', 0x0000, &[]));
        text.push('\n');
        text.into_bytes()
    }

    /// Encode one Intel HEX record with a correct checksum.
    fn ihex_line(length: u8, offset: u16, kind: u8, data: &[u8]) -> String {
        let mut record = vec![length];
        record.extend_from_slice(&offset.to_be_bytes());
        record.push(kind);
        record.extend_from_slice(data);
        let checksum = record
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte))
            .wrapping_neg();
        record.push(checksum);
        let mut line = String::from(":");
        for byte in &record {
            line.push_str(&format!("{byte:02X}"));
        }
        line
    }

    #[test]
    fn an_srec_image_loads_contiguous_records_as_one_segment() {
        let image = srec_image();
        let loaded = load_srec(&image).unwrap();
        assert_eq!(loaded.kind, ImageKind::Srec);
        assert_eq!(loaded.entry, Some(0));
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0);
        assert_eq!(
            loaded.segments[0].data,
            vec![0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE]
        );
    }

    #[test]
    fn a_corrupted_srec_checksum_is_rejected() {
        let mut image = srec_image();
        // Flip one data nibble in the first record.
        if let Some(position) = image.iter().position(|byte| *byte == b'D') {
            image[position] = b'E';
        }
        // The checksum no longer matches; the loader must reject the line
        // rather than load corrupted data.
        assert!(load_srec(&image).is_err());
    }

    #[test]
    fn an_ihex_image_loads_its_data_records() {
        let image = format!(
            "{}\n{}\n",
            ihex_line(4, 0x0000, 0x00, &[0xDE, 0xAD, 0xBE, 0xEF]),
            ihex_line(0, 0x0000, 0x01, &[])
        );
        let loaded = load_ihex(image.as_bytes()).unwrap();
        assert_eq!(loaded.kind, ImageKind::IntelHex);
        assert_eq!(loaded.segments.len(), 1);
        assert_eq!(loaded.segments[0].physical, 0);
        assert_eq!(loaded.segments[0].data, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn an_ihex_extended_linear_address_sets_the_upper_half() {
        let image = format!(
            "{}\n{}\n{}\n",
            ihex_line(2, 0x0000, 0x04, &[0x80, 0x00]),
            ihex_line(4, 0x0000, 0x00, &[0xDE, 0xAD, 0xBE, 0xEF]),
            ihex_line(0, 0x0000, 0x01, &[])
        );
        let loaded = load_ihex(image.as_bytes()).unwrap();
        assert_eq!(loaded.segments[0].physical, 0x8000_0000);
    }
}
