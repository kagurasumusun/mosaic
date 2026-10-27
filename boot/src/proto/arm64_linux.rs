//! The AArch64 Linux boot conventions (QEMU's `hw/arm/boot.c` AArch64 path,
//! decode side).
//!
//! Clean-room from `Documentation/arch/arm64/booting.rst`: the register
//! conventions (`x0` = physical address of the device tree blob, everything
//! else undefined), the placement rule (the image is loaded
//! `text_offset` bytes above a 2 MiB-aligned base within the first 512 MiB
//! of physical RAM, with `text_offset` read from the image header), and the
//! initramfs convention (page-aligned, outside the kernel's own footprint).
//!
//! The arm64 `Image` header (documented in the same file): a branch at word
//! 0, `text_offset` (u64 LE) at 0x08, `image_size` (u64 LE) at 0x10, and
//! `flags` at 0x18 (bit 3 = the image is position independent anywhere in
//! 4 GiB physical space).

/// The arm64 `Image` magic in the flags word (bits 31..32 = "Image").
pub const ARM64_IMAGE_MAGIC: u32 = 0x644D_5241; // "ARMd" — the spec's branch-first layout
/// Offset of `text_offset` in the header.
pub const TEXT_OFFSET_AT: usize = 0x08;
/// Offset of `image_size` in the header.
pub const IMAGE_SIZE_AT: usize = 0x10;
/// Offset of `flags` in the header.
pub const FLAGS_AT: usize = 0x18;
/// `flags` bit 3: the image is position independent anywhere in 4 GiB.
pub const FLAG_PIE_4G: u64 = 1 << 3;
/// Maximum distance from RAM base the kernel may be placed.
pub const MAX_PLACEMENT_ABOVE_BASE: u64 = 512 * 1024 * 1024;
/// The default `text_offset` for images built before the header carried it.
pub const LEGACY_TEXT_OFFSET: u64 = 0x0008_0000;
/// The conventional initramfs offset when the caller chooses none.
pub const INITRD_OFFSET: u64 = 0x0800_0000; // 128 MiB

/// The loader-side header facts of one arm64 `Image`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arm64Header {
    pub text_offset: u64,
    pub image_size: u64,
    pub flags: u64,
    pub position_independent_4g: bool,
}

/// Parse the arm64 `Image` header. Returns `None` when the image is too
/// short or the branch instruction at word 0 is not the `b` the
/// specification requires.
pub fn parse_header(image: &[u8]) -> Option<Arm64Header> {
    if image.len() < 0x20 {
        return None;
    }
    let u64le = |at: usize| {
        let mut word = [0u8; 8];
        word.copy_from_slice(&image[at..at + 8]);
        u64::from_le_bytes(word)
    };
    let first = u32::from_le_bytes([image[0], image[1], image[2], image[3]]);
    // A b-instruction has bits 31..26 = 000101.
    if first >> 26 != 0b000101 {
        return None;
    }
    let flags = u64le(FLAGS_AT);
    Some(Arm64Header {
        text_offset: u64le(TEXT_OFFSET_AT),
        image_size: u64le(IMAGE_SIZE_AT),
        flags,
        position_independent_4g: flags & FLAG_PIE_4G != 0,
    })
}

/// Facts the machine supplies for placement.
#[derive(Clone, Copy, Debug)]
pub struct Arm64Board {
    /// Physical base of RAM (2 MiB-aligned on real hardware; the plan aligns
    /// it itself to be safe).
    pub ram_base: u64,
    pub ram_size: u64,
    /// Where the machine places the device tree blob (the plan only reports
    /// `x0`; the DTB bytes are the machine's).
    pub dtb_address: u64,
}

/// The loader-side plan for one AArch64 Linux boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Arm64BootPlan {
    /// 2 MiB-aligned kernel placement base.
    pub kernel_base: u64,
    /// Where the kernel image bytes land: `kernel_base + text_offset`.
    pub kernel_load: u64,
    pub header: Arm64Header,
    /// `x0`: the DTB address.
    pub x0: u64,
    /// Page-aligned initramfs placement the caller should use.
    pub initrd_load: Option<u64>,
}

/// Build the boot plan from the header facts and the board's RAM layout.
pub fn plan(board: &Arm64Board, image: &[u8], initrd_size: Option<u64>) -> Option<Arm64BootPlan> {
    let header = parse_header(image)?;
    let base = board.ram_base & !0x1F_FFFF;
    if base
        .saturating_add(header.text_offset)
        .saturating_sub(board.ram_base)
        > MAX_PLACEMENT_ABOVE_BASE
    {
        return None;
    }
    let kernel_end = base
        .checked_add(header.text_offset)?
        .checked_add(header.image_size)?;
    let initrd_load = initrd_size.map(|size| {
        let candidate = (kernel_end + 0xFFF) & !0xFFF;
        if candidate.saturating_add(size) <= board.ram_base + board.ram_size {
            candidate
        } else {
            // Fall back to the conventional offset when the kernel's own
            // footprint would collide — the machine reports the collision if
            // it still does not fit.
            (board.ram_base + INITRD_OFFSET + 0xFFF) & !0xFFF
        }
    });
    Some(Arm64BootPlan {
        kernel_base: base,
        kernel_load: base + header.text_offset,
        header,
        x0: board.dtb_address,
        initrd_load,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm64 Image header with the legacy text_offset (a *format
    /// fixture*): `b .` at word 0, then the documented fields.
    fn image(text_offset: u64, image_size: u64) -> Vec<u8> {
        let mut image = vec![0u8; 0x20];
        // A `b` to the next instruction: imm26 = 0 is fine for parsing.
        image[0..4].copy_from_slice(&0b000101_00000000000000000000000000u32.to_le_bytes());
        image[TEXT_OFFSET_AT..TEXT_OFFSET_AT + 8].copy_from_slice(&text_offset.to_le_bytes());
        image[IMAGE_SIZE_AT..IMAGE_SIZE_AT + 8].copy_from_slice(&image_size.to_le_bytes());
        image
    }

    #[test]
    fn the_header_fields_are_decoded() {
        let header = parse_header(&image(0x80000, 0x1000)).unwrap();
        assert_eq!(header.text_offset, 0x0008_0000);
        assert_eq!(header.image_size, 0x1000);
        assert!(!header.position_independent_4g);
    }

    #[test]
    fn an_image_without_the_branch_is_rejected() {
        let mut image = image(0x80000, 0x1000);
        image[3] = 0xFF; // top byte: no longer a b-instruction
        assert!(parse_header(&image).is_none());
    }

    #[test]
    fn the_kernel_loads_at_the_aligned_base_plus_text_offset() {
        let board = Arm64Board {
            ram_base: 0x4000_0000,
            ram_size: 0x4000_0000,
            dtb_address: 0x4600_0000,
        };
        let plan = plan(&board, &image(0x80000, 0x1000), None).unwrap();
        assert_eq!(plan.kernel_base, 0x4000_0000);
        assert_eq!(plan.kernel_load, 0x4008_0000);
        assert_eq!(plan.x0, 0x4600_0000);
    }

    #[test]
    fn an_initramfs_lands_page_aligned_after_the_kernel() {
        let board = Arm64Board {
            ram_base: 0x4000_0000,
            ram_size: 0x4000_0000,
            dtb_address: 0x4600_0000,
        };
        let plan = plan(&board, &image(0x80000, 0x1234), Some(0x1000)).unwrap();
        let initrd = plan.initrd_load.unwrap();
        assert_eq!(initrd & 0xFFF, 0);
        assert_eq!(initrd, 0x4008_2000); // kernel end 0x4008_1234, next page
    }
}
