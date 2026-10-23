#![no_std]
#![forbid(unsafe_code)]

//! mosaic-prim-bits: L0 VM 概念を持たない純粋部品（設計書 §7 L0）。
//! バイト列への端序付き読み書きとビットフィールド抽出を純粋関数として
//! 提供します。アロケーションを行わず、短いバッファや不正な範囲指定は
//! エラーとして返すだけで、ゲスト入力でパニックすることはありません。

/// バイト列への読み書きやビットフィールド指定が不正な場合のエラー。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PrimError {
    /// 読み書き先バッファが要求された幅より短い。
    BufferTooSmall,
    /// ビットフィールドの `len` がゼロか、`start + len` が 64 ビットを超える。
    FieldRange,
}

/// u16 をリトルエンディアンで読む。
pub fn read_u16_le(bytes: &[u8]) -> Result<u16, PrimError> {
    if bytes.len() < 2 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// u16 をビッグエンディアンで読む。
pub fn read_u16_be(bytes: &[u8]) -> Result<u16, PrimError> {
    if bytes.len() < 2 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

/// u32 をリトルエンディアンで読む。
pub fn read_u32_le(bytes: &[u8]) -> Result<u32, PrimError> {
    if bytes.len() < 4 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// u32 をビッグエンディアンで読む。
pub fn read_u32_be(bytes: &[u8]) -> Result<u32, PrimError> {
    if bytes.len() < 4 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// u64 をリトルエンディアンで読む。
pub fn read_u64_le(bytes: &[u8]) -> Result<u64, PrimError> {
    if bytes.len() < 8 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

/// u64 をビッグエンディアンで読む。
pub fn read_u64_be(bytes: &[u8]) -> Result<u64, PrimError> {
    if bytes.len() < 8 {
        return Err(PrimError::BufferTooSmall);
    }
    Ok(u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

/// u16 をリトルエンディアンで `buf` の先頭に書き込む（`buf` は 2 バイト以上）。
pub fn write_u16_le(
    buf: &mut [u8],
    value: u16,
) -> Result<(), PrimError> {
    if buf.len() < 2 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_le_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    Ok(())
}

/// u16 をビッグエンディアンで `buf` の先頭に書き込む（`buf` は 2 バイト以上）。
pub fn write_u16_be(
    buf: &mut [u8],
    value: u16,
) -> Result<(), PrimError> {
    if buf.len() < 2 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_be_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    Ok(())
}

/// u32 をリトルエンディアンで `buf` の先頭に書き込む（`buf` は 4 バイト以上）。
pub fn write_u32_le(
    buf: &mut [u8],
    value: u32,
) -> Result<(), PrimError> {
    if buf.len() < 4 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_le_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    buf[2] = bytes[2];
    buf[3] = bytes[3];
    Ok(())
}

/// u32 をビッグエンディアンで `buf` の先頭に書き込む（`buf` は 4 バイト以上）。
pub fn write_u32_be(
    buf: &mut [u8],
    value: u32,
) -> Result<(), PrimError> {
    if buf.len() < 4 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_be_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    buf[2] = bytes[2];
    buf[3] = bytes[3];
    Ok(())
}

/// u64 をリトルエンディアンで `buf` の先頭に書き込む（`buf` は 8 バイト以上）。
pub fn write_u64_le(
    buf: &mut [u8],
    value: u64,
) -> Result<(), PrimError> {
    if buf.len() < 8 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_le_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    buf[2] = bytes[2];
    buf[3] = bytes[3];
    buf[4] = bytes[4];
    buf[5] = bytes[5];
    buf[6] = bytes[6];
    buf[7] = bytes[7];
    Ok(())
}

/// u64 をビッグエンディアンで `buf` の先頭に書き込む（`buf` は 8 バイト以上）。
pub fn write_u64_be(
    buf: &mut [u8],
    value: u64,
) -> Result<(), PrimError> {
    if buf.len() < 8 {
        return Err(PrimError::BufferTooSmall);
    }
    let bytes = value.to_be_bytes();
    buf[0] = bytes[0];
    buf[1] = bytes[1];
    buf[2] = bytes[2];
    buf[3] = bytes[3];
    buf[4] = bytes[4];
    buf[5] = bytes[5];
    buf[6] = bytes[6];
    buf[7] = bytes[7];
    Ok(())
}

/// `value` のビット `start` から `len` ビットを抽出する。
/// `len` は 1..=64 で `start + len <= 64` でなければ `FieldRange` を返す。
pub fn field(value: u64, start: u8, len: u8) -> Result<u64, PrimError> {
    let shift = u32::from(start);
    let width = u32::from(len);
    if width == 0 || shift + width > 64 {
        return Err(PrimError::FieldRange);
    }
    let mask = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
    Ok((value >> shift) & mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u16_round_trip_in_both_endians() {
        let mut buf = [0u8; 4];
        write_u16_le(&mut buf, 0x1234).unwrap();
        assert_eq!(&buf[..2], &[0x34, 0x12]);
        assert_eq!(read_u16_le(&buf[..2]), Ok(0x1234));
        assert_eq!(read_u16_be(&buf[..2]), Ok(0x3412));

        write_u16_be(&mut buf, 0x1234).unwrap();
        assert_eq!(&buf[..2], &[0x12, 0x34]);
        assert_eq!(read_u16_be(&buf[..2]), Ok(0x1234));
        assert_eq!(read_u16_le(&buf[..2]), Ok(0x3412));
    }

    #[test]
    fn u32_round_trip_in_both_endians() {
        let mut buf = [0u8; 8];
        write_u32_le(&mut buf, 0x1122_3344).unwrap();
        assert_eq!(&buf[..4], &[0x44, 0x33, 0x22, 0x11]);
        assert_eq!(read_u32_le(&buf[..4]), Ok(0x1122_3344));

        write_u32_be(&mut buf[4..], 0x1122_3344).unwrap();
        assert_eq!(&buf[4..], &[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(read_u32_be(&buf[4..]), Ok(0x1122_3344));
    }

    #[test]
    fn u64_round_trip_in_both_endians() {
        let mut buf = [0u8; 16];
        let value = 0x0123_4567_89ab_cdef;
        write_u64_le(&mut buf, value).unwrap();
        assert_eq!(&buf[..8], &[0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01]);
        assert_eq!(read_u64_le(&buf[..8]), Ok(value));

        write_u64_be(&mut buf[8..], value).unwrap();
        assert_eq!(&buf[8..], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef]);
        assert_eq!(read_u64_be(&buf[8..]), Ok(value));
    }

    #[test]
    fn short_buffer_is_reported_without_panic() {
        assert_eq!(read_u32_le(&[0, 1, 2]), Err(PrimError::BufferTooSmall));
        assert_eq!(read_u64_be(&[0; 7]), Err(PrimError::BufferTooSmall));
        assert_eq!(write_u64_le(&mut [0; 7], 0), Err(PrimError::BufferTooSmall));
        assert_eq!(write_u32_be(&mut [0; 3], 0), Err(PrimError::BufferTooSmall));
    }

    #[test]
    fn field_extracts_bits_without_panic_on_range_errors() {
        assert_eq!(field(0b_1011_0010, 1, 4), Ok(9));
        assert_eq!(field(0xFF, 4, 4), Ok(0x0F));
        assert_eq!(field(u64::MAX, 0, 64), Ok(u64::MAX));
        assert_eq!(field(u64::MAX, 32, 32), Ok(0xFFFF_FFFF));
        assert_eq!(field(0, 0, 0), Err(PrimError::FieldRange));
        assert_eq!(field(0, 60, 5), Err(PrimError::FieldRange));
        assert_eq!(field(0, 63, 2), Err(PrimError::FieldRange));
    }
}
