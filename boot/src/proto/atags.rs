//! ATAGS construction (the ARM32 Linux boot convention, pre-device-tree).
//!
//! Clean-room from `Documentation/arch/arm/booting.rst` and the ATAG
//! definition in the kernel's `arch/arm/include/uapi/asm/setup.h`: a list of
//! `{size, tag}` headers with tag payloads, word-sized, null-terminated, that
//! a bootloader passes at `r2` (or in `dtb` space when a device tree is
//! used instead — never both).
//!
//! The builder produces exactly the tags the kernel documents; nothing here
//! invents tags the kernel would ignore.

/// The ATAG word: `(size_in_words_including_header) << 2 | tag_number`.
/// Tag numbers the Linux kernel defines.
pub mod tag {
    pub const NONE: u32 = 0x0000_0000;
    pub const CORE: u32 = 0x5441_0001;
    pub const MEM: u32 = 0x5441_0002;
    pub const SERIAL: u32 = 0x5441_0006;
    pub const REVISION: u32 = 0x5441_0007;
    pub const VIDEOTEXT: u32 = 0x5441_0003;
    pub const RAMDISK: u32 = 0x5441_0004;
    pub const INITRD2: u32 = 0x5442_0005;
    pub const CMDLINE: u32 = 0x5441_0009;
}

/// One ATAG's words, ready to write into the tag list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Atag {
    pub tag: u32,
    pub payload: Vec<u32>,
}

impl Atag {
    /// `ATAG_CORE`: flags and page size (page size 0 = device-tree era
    /// convention).
    pub fn core(page_size: u32, root_device: u32) -> Self {
        Self {
            tag: tag::CORE,
            payload: vec![0x0000_0000, page_size, root_device],
        }
    }

    /// `ATAG_MEM`: one memory bank's start and size.
    pub fn mem(start: u32, size: u32) -> Self {
        Self {
            tag: tag::MEM,
            payload: vec![size, start],
        }
    }

    /// `ATAG_CMDLINE`: the kernel command line, NUL-padded to word size.
    pub fn cmdline(line: &str) -> Self {
        let mut bytes = line.as_bytes().to_vec();
        bytes.push(0);
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let payload = bytes
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        Self {
            tag: tag::CMDLINE,
            payload,
        }
    }

    /// `ATAG_RAMDISK` / `ATAG_INITRD2`: an already-loaded initramfs.
    pub fn initrd(start: u32, size: u32) -> Self {
        Self {
            tag: tag::INITRD2,
            payload: vec![start, size],
        }
    }

    /// `ATAG_SERIAL`: the board's 64-bit serial number.
    pub fn serial(low: u32, high: u32) -> Self {
        Self {
            tag: tag::SERIAL,
            payload: vec![low, high],
        }
    }

    /// `ATAG_REVISION`: the board revision.
    pub fn revision(value: u32) -> Self {
        Self {
            tag: tag::REVISION,
            payload: vec![value],
        }
    }

    fn words(&self) -> usize {
        2 + self.payload.len()
    }
}

/// Build one ATAG list, terminated by `ATAG_NONE`.
///
/// Each header's first word is the tag's size in 32-bit words *including
/// the two-word header* — the exact layout the kernel's `tag_next` walks —
/// and the second word is the tag number. The list is word-aligned; the
/// caller places it at the conventional `0x100` offset above the RAM base
/// (`arm_linux::ATAGS_OFFSET`).
pub fn build(atags: &[Atag]) -> Vec<u8> {
    let mut words = Vec::new();
    for atag in atags {
        words.push(atag.words() as u32);
        words.push(atag.tag);
        words.extend_from_slice(&atag.payload);
    }
    words.push(0x0000_0000); // ATAG_NONE: size zero ends the walk
    words.push(tag::NONE);
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_word_sized_and_terminated() {
        let bytes = build(&[Atag::core(0, 0), Atag::mem(0, 0x0200_0000)]);
        assert_eq!(bytes.len() % 4, 0);
        let last = bytes.len();
        let terminator = u32::from_le_bytes([
            bytes[last - 8],
            bytes[last - 7],
            bytes[last - 6],
            bytes[last - 5],
        ]);
        assert_eq!(terminator, 0); // ATAG_NONE: size 0
    }

    #[test]
    fn a_core_tag_carries_the_documented_header_and_payload() {
        let bytes = build(&[Atag::core(0, 0)]);
        let size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let tag_value = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        // 2 header words + 3 payload words = 5, the size the kernel's
        // `tag_next` advances; the tag number follows as its own word.
        assert_eq!(size, 5);
        assert_eq!(tag_value, tag::CORE);
    }

    #[test]
    fn a_cmdline_tag_is_nul_padded_to_words() {
        let bytes = build(&[Atag::cmdline("console=ttyAMA0")]);
        // 16-byte command + NUL = 17 bytes, padded to 20.
        let payload_at = 8;
        let text = &bytes[payload_at..];
        let until_nul = text
            .iter()
            .position(|byte| *byte == 0)
            .expect("NUL inside the tag");
        assert_eq!(&text[..until_nul], b"console=ttyAMA0");
        assert_eq!((until_nul + 1) % 4, 0);
    }
}
