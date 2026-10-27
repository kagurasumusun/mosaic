//! The ARM32 Linux boot conventions (QEMU's `hw/arm/boot.c` role, decode
//! side).
//!
//! Clean-room from `Documentation/arch/arm/booting.rst`: the register
//! conventions (`r0 = 0`, `r1 = machine number`, `r2 = ATAGS or DTB
//! pointer`), the placement rules (a raw `Image` loads 32 KiB into RAM; a
//! self-decompressing `zImage` may sit anywhere reasonable, by convention
//! loaded at the base), and the initramfs convention (page-aligned above the
//! kernel, at the start of RAM plus 8 MiB when the caller has no better
//! address).

use super::atags::{self, Atag};

/// Offset of a raw `Image`'s load address above the RAM base.
pub const IMAGE_OFFSET: u64 = 0x0000_8000;
/// Where a `zImage` is conventionally loaded (above the ATAGS space and the
/// first pages the kernel zeroes).
pub const ZIMAGE_OFFSET: u64 = 0x0001_0000;
/// Offset of the ATAGS/parameter list above the RAM base.
pub const ATAGS_OFFSET: u64 = 0x0000_0100;
/// Offset of the initramfs above the RAM base when the caller chooses none.
pub const INITRD_OFFSET: u64 = 0x0080_0000;
/// The Linux `Image` magic ("ARM\x16") at the image's 32-byte offset.
pub const IMAGE_MAGIC: u32 = 0x016F_2818;
/// Offset of the magic in a raw `Image`.
pub const IMAGE_MAGIC_OFFSET: usize = 0x24;

/// What the image handed to [`plan`] turned out to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArmImageKind {
    /// A raw (vmlinux-format) `Image` with the `ARM\x16` magic.
    RawImage,
    /// A self-decompressing `zImage` (legacy uImage payloads, etc.).
    ZImage,
    /// An image carrying its own multiboot/uImage container facts — the
    /// caller resolves it through `loader` first.
    Container,
}

/// The loader-side plan for one ARM32 Linux boot: where every image lands
/// and what the registers carry. Placement into guest memory stays with the
/// machine model; this is the protocol's answer, not a machine's.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArmBootPlan {
    pub kind: ArmImageKind,
    /// Load address of the kernel image (RAM-relative absolute).
    pub kernel_load: u64,
    /// `r1`: the machine number (ATAGS-era boards) or zero.
    pub machine_number: u32,
    /// `r2`: the ATAGS list address (built here) or the DTB address.
    pub atags_address: u64,
    /// The ATAGS bytes to place at `atags_address`.
    pub atags: Vec<u8>,
    /// Optional initramfs: `(load address, bytes)`.
    pub initrd: Option<(u64, Vec<u8>)>,
}

/// Facts the machine supplies: where RAM starts and how much there is, the
/// machine number, the command line.
#[derive(Clone, Debug)]
pub struct ArmBoard {
    /// Physical base of RAM (typically zero on ARM32 boards).
    pub ram_base: u64,
    pub ram_size: u64,
    /// The machine number `r1` carries (ATAGS-era boards; zero for DT).
    pub machine_number: u32,
    pub cmdline: String,
}

/// Recognize the image shape by its leading bytes.
pub fn image_kind(image: &[u8]) -> ArmImageKind {
    if image.len() >= IMAGE_MAGIC_OFFSET + 4 {
        let magic = u32::from_le_bytes([
            image[IMAGE_MAGIC_OFFSET],
            image[IMAGE_MAGIC_OFFSET + 1],
            image[IMAGE_MAGIC_OFFSET + 2],
            image[IMAGE_MAGIC_OFFSET + 3],
        ]);
        if magic == IMAGE_MAGIC {
            return ArmImageKind::RawImage;
        }
    }
    ArmImageKind::ZImage
}

/// Build the boot plan: kernel placement, ATAGS (memory, command line,
/// initramfs) and the initramfs address.
pub fn plan(board: &ArmBoard, kernel: &[u8], initrd: Option<&[u8]>) -> ArmBootPlan {
    let kind = image_kind(kernel);
    let kernel_load = board.ram_base
        + match kind {
            ArmImageKind::RawImage => IMAGE_OFFSET,
            _ => ZIMAGE_OFFSET,
        };
    let initrd_placement = initrd.map(|bytes| {
        let address = board.ram_base + INITRD_OFFSET;
        let aligned = (address + 0xFFF) & !0xFFF;
        (aligned, bytes.to_vec())
    });
    let mut tags = vec![
        Atag::core(0, 0),
        Atag::mem(
            u32::try_from(board.ram_base).unwrap_or(0),
            u32::try_from(board.ram_size).unwrap_or(u32::MAX),
        ),
    ];
    if let Some((address, bytes)) = &initrd_placement {
        tags.push(Atag::initrd(
            u32::try_from(*address).unwrap_or(0),
            u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        ));
    }
    if !board.cmdline.is_empty() {
        tags.push(Atag::cmdline(&board.cmdline));
    }
    ArmBootPlan {
        kind,
        kernel_load,
        machine_number: board.machine_number,
        atags_address: board.ram_base + ATAGS_OFFSET,
        atags: atags::build(&tags),
        initrd: initrd_placement,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> ArmBoard {
        ArmBoard {
            ram_base: 0,
            ram_size: 0x0800_0000, // 128 MiB
            machine_number: 0x0000_04FC,
            cmdline: "console=ttyAMA0".into(),
        }
    }

    #[test]
    fn a_raw_image_loads_at_the_documented_offset() {
        let board = board();
        let mut image = vec![0u8; 0x28];
        image[IMAGE_MAGIC_OFFSET..IMAGE_MAGIC_OFFSET + 4]
            .copy_from_slice(&IMAGE_MAGIC.to_le_bytes());
        let plan = plan(&board, &image, None);
        assert_eq!(plan.kind, ArmImageKind::RawImage);
        assert_eq!(plan.kernel_load, 0x0000_8000);
        assert_eq!(plan.atags_address, ATAGS_OFFSET);
        assert_eq!(plan.machine_number, 0x04FC);
    }

    #[test]
    fn a_zimage_loads_at_the_conventional_offset() {
        let board = board();
        let plan = plan(&board, b"zimage-bytes", None);
        assert_eq!(plan.kind, ArmImageKind::ZImage);
        assert_eq!(plan.kernel_load, ZIMAGE_OFFSET);
    }

    #[test]
    fn an_initramfs_is_page_aligned_and_tagged() {
        let board = board();
        let plan = plan(&board, b"zimage", Some(&[0u8; 300]));
        let (address, bytes) = plan.initrd.expect("initrd");
        assert_eq!(address, 0x0080_0000);
        assert_eq!(bytes.len(), 300);
        // The ATAGS carry an INITRD2 tag with the placement.
        let words: Vec<u32> = plan
            .atags
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        let initrd_tag = words
            .windows(4)
            .any(|window| window[0] == 4 && window[1] == atags::tag::INITRD2);
        assert!(initrd_tag);
    }
}
