//! Independent T32 image for the STM32F103 slice.
//!
//! The literal pool carries the PA5 CRL value `0x4434_4444` (nibble 5 is the
//! general-purpose output), the USART BRR, CRH reset, and the SCS addresses
//! the guest polls. The hex printer shifts the high nibble out first and adds
//! `0x37` for A–F. Vector word 0 is the real initial SP.

use crate::board::{SRAM_BASE, SRAM_SIZE};

/// Initial stack pointer: top of SRAM (vector-table word 0).
pub const INITIAL_SP: u64 = SRAM_BASE + SRAM_SIZE;
/// Reset handler entry (vector-table word 1 points here with bit 0 set).
pub const RESET_HANDLER: u64 = 0x0000_0008;

/// Expected golden console output for the F103 firmware.
pub const GOLDEN_OUTPUT: &[u8] =
    b"mosaic e=00004004 s=000000C0 o=00000020 p=00000020 i=00435000 q=00000000\n";

/// Observed result of one full firmware run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stm32F103Report {
    pub output: Vec<u8>,
    pub hooks: u64,
    pub events: usize,
    pub pin_edges: Vec<bool>,
}

const FIRMWARE_WORDS: [u32; 85] = [
    0x20005000, 0x00000009, 0x210469A8, 0x21404308, 0x43080209, 0x484B61A8, 0x48486070, 0x20206030,
    0x48476130, 0x200860A0, 0x02092120, 0x60E04308, 0xF000216D, 0x216FF86E, 0xF86BF000, 0xF0002173,
    0x2161F868, 0xF865F000, 0xF0002169, 0x2163F862, 0xF85FF000, 0xF0002120, 0x2165F85C, 0xF859F000,
    0xF000213D, 0x69A8F856, 0xF858F000, 0xF0002120, 0x2173F850, 0xF84DF000, 0xF000213D, 0x6820F84A,
    0xF84CF000, 0xF0002120, 0x216FF844, 0xF841F000, 0xF000213D, 0x68F0F83E, 0xF840F000, 0x2120482A,
    0x68E06001, 0x43082180, 0x482760E0, 0x28206800, 0x2120D1FB, 0xF82DF000, 0xF0002170, 0x213DF82A,
    0xF827F000, 0xF82AF000, 0xF0002120, 0x2169F822, 0xF81FF000, 0xF000213D, 0x481DF81C, 0xF0006800,
    0x68E0F81D, 0x40012180, 0x60E04048, 0x68004817, 0xD1FB2800, 0xF0002120, 0x2171F80C, 0xF809F000,
    0xF000213D, 0xF000F806, 0x210AF809, 0xF801F000, 0x6823E7FE, 0xD5FC061B, 0x47707121, 0x2208B500,
    0x01000F01, 0x290ABF00, 0x3137D301, 0x3130E000, 0xFFEFF7FF, 0xD1F33A01, 0x0000BD00, 0x44344444,
    0x00000271, 0x44444444, 0xE000E104, 0xE000E204, 0xE000ED04,
];

pub fn firmware() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FIRMWARE_WORDS.len() * 4);
    for word in FIRMWARE_WORDS {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_vector_table_and_entry() {
        let image = firmware();
        assert_eq!(image.len(), FIRMWARE_WORDS.len() * 4);
        let word = |offset: usize| -> u32 {
            u32::from_le_bytes(image[offset..offset + 4].try_into().expect("vector word"))
        };
        assert_eq!(word(0), INITIAL_SP as u32);
        assert_eq!(word(4), 0x0000_0009);
    }
}
