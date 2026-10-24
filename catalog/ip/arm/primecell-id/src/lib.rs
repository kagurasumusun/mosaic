#![no_std]
#![forbid(unsafe_code)]
//! ARM PrimeCell peripheral identification block.
//!
//! Every ARM PrimeCell exposes four 32-bit peripheral identification
//! registers at offsets `0xFE0..=0xFEC` and four PrimeCell identification
//! registers at `0xFF0..=0xFFC` (ARM DDI 0273 peripheral ID scheme). This
//! crate is an `ip-lib`: it holds the shared encoding and no device
//! registration, so any PrimeCell model (`pl011`, `sp804`, ...) can depend on
//! it without creating catalog cross-dependencies between models.

/// Width of one identification window: eight 4-byte registers.
pub const ID_WINDOW_BASE: u64 = 0xFE0;
pub const ID_WINDOW_END: u64 = 0xFFC;

/// Fixed PrimeCell identification bytes spelling the ARM cell signature.
pub const CELL_ID: [u8; 4] = [0x0D, 0xF0, 0x05, 0xB1];

/// JEDEC identity code for Arm Limited, used by every ARM PrimeCell.
pub const ARM_DESIGNER: u8 = 0x41;

/// Field-validated PrimeCell identity. Construct through [`PrimeCellId::new`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimeCellId {
    part_number: u16,
    designer: u8,
    revision: u8,
    patch: u8,
}

impl PrimeCellId {
    /// Validate the field widths used by the peripheral ID registers.
    pub const fn new(part_number: u16, designer: u8, revision: u8, patch: u8) -> Option<Self> {
        if part_number > 0xFFF || designer > 0x7F || revision > 0x7 || patch > 0xF {
            return None;
        }
        Some(Self {
            part_number,
            designer,
            revision,
            patch,
        })
    }

    /// Identity block for an ARM-designed PrimeCell (`designer = 0x41`).
    pub const fn arm(part_number: u16, revision: u8, patch: u8) -> Option<Self> {
        Self::new(part_number, ARM_DESIGNER, revision, patch)
    }

    /// True for the eight word-aligned identification register offsets.
    pub const fn covers(offset: u64) -> bool {
        offset >= ID_WINDOW_BASE && offset <= ID_WINDOW_END && (offset - ID_WINDOW_BASE) % 4 == 0
    }

    /// Read one identification word. Returns `None` outside the window so the
    /// owning model applies its own unmapped-register policy.
    pub const fn read_word(&self, offset: u64) -> Option<u32> {
        if !Self::covers(offset) {
            return None;
        }
        let byte = match offset {
            0xFE0 => (self.part_number & 0xFF) as u8,
            0xFE4 => {
                let part_high = ((self.part_number >> 8) & 0x0F) as u8;
                let designer_low = (self.designer & 0x0F) << 4;
                part_high | designer_low
            }
            0xFEC => {
                let designer_high = (self.designer >> 4) & 0x0F;
                let revision = (self.revision & 0x07) << 4;
                // Bit 7 marks a JEDEC-coded designer identity.
                designer_high | revision | 0x80
            }
            0xFF0 => CELL_ID[0],
            0xFF4 => CELL_ID[1],
            0xFF8 => CELL_ID[2],
            0xFFC => CELL_ID[3],
            _ => return None,
        };
        Some(byte as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_pl011_primecell_identity() {
        // PL011 part number 0x011, ARM designer, first revision, no patch.
        let id = PrimeCellId::arm(0x011, 0, 0).unwrap();
        assert_eq!(id.read_word(0xFE0), Some(0x11));
        // PID1 = part[11:8] | JEP104 identity[3:0] << 4 = 0x0 | 0x1 << 4.
        assert_eq!(id.read_word(0xFE4), Some(0x10));
        assert_eq!(id.read_word(0xFEC), Some(0x84));
        assert_eq!(id.read_word(0xFF0), Some(0x0D));
        assert_eq!(id.read_word(0xFF4), Some(0xF0));
        assert_eq!(id.read_word(0xFF8), Some(0x05));
        assert_eq!(id.read_word(0xFFC), Some(0xB1));
    }

    #[test]
    fn revision_and_part_fields_stay_inside_their_bit_widths() {
        let id = PrimeCellId::arm(0xFFF, 7, 0xF).unwrap();
        assert_eq!(id.read_word(0xFE0), Some(0xFF));
        assert_eq!(id.read_word(0xFE4), Some(0x1F));
        assert_eq!(id.read_word(0xFEC), Some(0xF4));
        assert!(PrimeCellId::new(0x1000, 0, 0, 0).is_none());
        assert!(PrimeCellId::new(0, 0x80, 0, 0).is_none());
        assert!(PrimeCellId::new(0, 0, 8, 0).is_none());
        assert!(PrimeCellId::new(0, 0, 0, 0x10).is_none());
    }

    #[test]
    fn only_the_word_aligned_identification_window_is_covered() {
        let id = PrimeCellId::arm(1, 0, 0).unwrap();
        assert!(PrimeCellId::covers(0xFE0));
        assert!(PrimeCellId::covers(0xFFC));
        assert!(!PrimeCellId::covers(0xFDC));
        assert!(!PrimeCellId::covers(0x1000));
        assert!(!PrimeCellId::covers(0xFE2));
        assert_eq!(id.read_word(0x1000), None);
    }
}
