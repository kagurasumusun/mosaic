//! Stable FNV-1a 64-bit hasher for in-process fingerprints.
//!
//! This is a compatibility checksum, not a cryptographic digest. Field order
//! and little-endian integer encoding are part of the fingerprint contract.

/// FNV-1a 64-bit accumulator. The offset basis and prime match the runtime
/// graph fingerprint so both can be compared as the same family of checksums.
#[derive(Clone, Debug)]
pub struct Fnv64(u64);

impl Fnv64 {
    pub const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub fn byte(&mut self, byte: u8) {
        self.0 ^= u64::from(byte);
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }

    pub fn bytes(&mut self, data: &[u8]) {
        for byte in data {
            self.byte(*byte);
        }
    }

    pub fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    pub fn i16(&mut self, value: i16) {
        self.u16(value as u16);
    }

    pub fn i32(&mut self, value: i32) {
        self.u32(value as u32);
    }

    pub fn usize(&mut self, value: usize) {
        self.u64(value as u64);
    }

    pub fn bool(&mut self, value: bool) {
        self.byte(u8::from(value));
    }

    pub fn option_u64(&mut self, value: Option<u64>) {
        match value {
            None => self.byte(0),
            Some(inner) => {
                self.byte(1);
                self.u64(inner);
            }
        }
    }

    pub const fn finish(self) -> u64 {
        self.0
    }
}

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}
