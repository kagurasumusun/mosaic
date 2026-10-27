//! The RISC-V Linux boot conventions (QEMU's `hw/riscv/boot.c` role,
//! decode side).
//!
//! Clean-room from `Documentation/arch/riscv/boot.rst` (and the supervisor
//! binary interface): the hart whose ID is in `a0` enters the kernel at the
//! image's entry, `a1` carries the device tree blob's physical address, the
//! kernel image is placed on a 2 MiB-aligned address at least 2 MiB above
//! the RAM base (QEMU's `riscv_load_kernel` convention), and the initramfs
//! is page-aligned beyond the kernel image.
//!
//! A RISC-V kernel image is a raw flat binary or an ELF; both facts come
//! from the [`crate::loader`], this module owns the placement and register
//! conventions only.

/// Kernel placement is 2 MiB-aligned, at least this far above the RAM base.
pub const KERNEL_ALIGNMENT: u64 = 0x0020_0000;
/// Minimum offset above the RAM base (the kernel's own early mapping space).
pub const KERNEL_MIN_OFFSET: u64 = 0x0020_0000;
/// Conventional initramfs offset when the caller chooses none.
pub const INITRD_OFFSET: u64 = 0x0400_0000; // 64 MiB

/// Facts the machine supplies for placement.
#[derive(Clone, Copy, Debug)]
pub struct RiscvBoard {
    pub ram_base: u64,
    pub ram_size: u64,
    /// Where the machine places the device tree blob (`a1`).
    pub dtb_address: u64,
}

/// The loader-side plan for one RISC-V Linux boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiscvBootPlan {
    /// Where the kernel image bytes land.
    pub kernel_load: u64,
    /// `a0`: the booting hart's ID (the machine's CPU numbering).
    pub hart_id: u64,
    /// `a1`: the DTB address.
    pub dtb_address: u64,
    /// Page-aligned initramfs placement, when one is loaded.
    pub initrd_load: Option<u64>,
}

/// Build the boot plan. `kernel_size` and `initrd_size` are byte counts from
/// the loader; the plan never reads the bytes itself.
pub fn plan(
    board: &RiscvBoard,
    hart_id: u64,
    kernel_size: u64,
    initrd_size: Option<u64>,
) -> RiscvBootPlan {
    let kernel_load = align_up(board.ram_base + KERNEL_MIN_OFFSET, KERNEL_ALIGNMENT);
    let initrd_load = initrd_size.map(|size| {
        let after_kernel = align_up(kernel_load + kernel_size, 0x1000);
        let candidate = after_kernel.max(board.ram_base.saturating_add(INITRD_OFFSET));
        let _ = size;
        candidate
    });
    RiscvBootPlan {
        kernel_load,
        hart_id,
        dtb_address: board.dtb_address,
        initrd_load,
    }
}

fn align_up(value: u64, alignment: u64) -> u64 {
    (value + alignment - 1) & !(alignment - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_loads_2mb_aligned_above_the_ram_base() {
        let board = RiscvBoard {
            ram_base: 0x8000_0000,
            ram_size: 0x1000_0000,
            dtb_address: 0x87E0_0000,
        };
        let plan = plan(&board, 0, 0x0010_0000, None);
        assert_eq!(plan.kernel_load, 0x8020_0000);
        assert_eq!(plan.kernel_load % KERNEL_ALIGNMENT, 0);
        assert_eq!(plan.hart_id, 0);
        assert_eq!(plan.dtb_address, 0x87E0_0000);
    }

    #[test]
    fn an_initramfs_lands_page_aligned() {
        let board = RiscvBoard {
            ram_base: 0x8000_0000,
            ram_size: 0x1000_0000,
            dtb_address: 0x87E0_0000,
        };
        let plan = plan(&board, 1, 0x0010_0000, Some(0x2000));
        let initrd = plan.initrd_load.unwrap();
        assert_eq!(initrd % 0x1000, 0);
        // At least the conventional 64 MiB offset, or just past the kernel.
        assert!(initrd >= 0x8030_0000);
        assert_eq!(plan.hart_id, 1);
    }
}
