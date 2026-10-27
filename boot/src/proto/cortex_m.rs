//! The Cortex-M reset sequence (QEMU's `armv7m_load_kernel` vector-table
//! read).
//!
//! A Cortex-M image begins with its vector table: word 0 is the initial
//! stack pointer (full-descending, word-aligned), word 1 is the reset
//! handler with the Thumb bit set. No loader may preload anything else into
//! registers — the reset sequence is exactly these two words.

/// Read the Cortex-M reset entry (initial SP, reset-handler PC) out of an
/// image's own vector table.
///
/// The Thumb bit (bit 0 of word 1) is masked off: the core always executes
/// Thumb on Cortex-M, and the entry is the aligned handler address.
pub fn vector_entry(image: &[u8]) -> Result<(u64, u64), String> {
    let word = |offset: usize| -> Result<u32, String> {
        image
            .get(offset..offset + 4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or_else(|| "image is shorter than a Cortex-M vector table".to_string())
    };
    let sp = word(0)?;
    let pc = word(4)? & !1;
    if sp == 0 || pc == 0 {
        return Err("vector table entry is null".into());
    }
    // The initial SP must be word aligned and (on real hardware) inside RAM;
    // alignment is checked here, placement is the machine's to know.
    if sp & 0x3 != 0 {
        return Err("initial SP is not word aligned".into());
    }
    Ok((u64::from(sp), u64::from(pc)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_entry_reads_sp_and_pc_like_the_reset_sequence() {
        let mut image = vec![0u8; 16];
        image[0..4].copy_from_slice(&0x2000_5000u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0000_0009u32.to_le_bytes());
        assert_eq!(vector_entry(&image), Ok((0x2000_5000, 0x0000_0008)));
    }

    #[test]
    fn vector_entry_rejects_a_table_that_is_not_there() {
        assert!(vector_entry(&[0u8; 4]).is_err());
        assert!(vector_entry(&[0u8; 8]).is_err());
    }

    #[test]
    fn a_misaligned_stack_pointer_is_rejected() {
        let mut image = vec![0u8; 16];
        image[0..4].copy_from_slice(&0x2000_5002u32.to_le_bytes());
        image[4..8].copy_from_slice(&0x0000_0009u32.to_le_bytes());
        assert!(vector_entry(&image).is_err());
    }
}
